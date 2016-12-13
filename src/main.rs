#[macro_use]
extern crate clap;
extern crate git2;
extern crate secret_service;
extern crate schedule_recv;
extern crate walkdir;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::Duration;

use clap::{App, Arg, ArgMatches, SubCommand};

use git2::{Error, FetchOptions, PushOptions, Repository, RemoteCallbacks};
use git2::build::RepoBuilder;

use secret_service::SecretService;
use secret_service::EncryptionType;

use walkdir::{DirEntry, WalkDir, WalkDirIterator};

const STORE_NAME: &'static str = ".snowflakes";

fn main() {
    let matches = App::new("flake")
        .version("1.0")
        .author("David Calavera <david.calavera@gmail.com>")
        .about("Keep track of dotfiles")
        .subcommand(SubCommand::with_name("auth")
            .about("Store auth token in the credentials store")
            .arg(Arg::with_name("token")
                .required(true)
                .help("GitHub's access token")))
        .subcommand(SubCommand::with_name("sync")
            .about("Syncronize repository")
            .arg(Arg::with_name("repository")
                .short("r")
                .long("repository")
                .value_name("HTTP_URL")
                .help("The repository http url"))
            .arg(Arg::with_name("interval")
                .short("i")
                .long("interval")
                .value_name("SECONDS")
                .help("The interval to sync files in seconds")))
        .get_matches();

    match matches.subcommand() {
        ("auth", Some(auth_matches)) => auth(auth_matches),
        ("sync", Some(sync_matches)) => sync(sync_matches),
        ("", None) => println!("Please, run flake command with `auth` or `sync` subcommands"),
        _ => unreachable!(),
    }
}

fn auth(matches: &ArgMatches) {
    match SecretService::new(EncryptionType::Dh) {
        Err(error) => {
            println!("Unable to connect with the secret service: {}", error);
            process::exit(1);
        }
        Ok(ss) => {
            let collection = ss.get_default_collection().unwrap();

            let token = String::from(matches.value_of("token").unwrap());
            if let Err(error) = collection.create_item("flake",
                                                       vec![("github", "access_token")],
                                                       token.as_bytes(),
                                                       true,
                                                       "text/plain") {
                println!("Something went wrong saving the access token :/ {}", error);
                process::exit(1);
            }
        }
    }
}

fn sync(matches: &ArgMatches) {
    let config = git2::Config::open_default().unwrap().snapshot().unwrap();
    let url = match matches.value_of("repository") {
        None => {
            match config.get_str("github.dotfiles") {
                Err(error) => {
                    println!("repository url not provided, use `git config --global --add \
                              github.dotfiles URL` to set a default repository: {}",
                             error);
                    process::exit(1);
                }
                Ok(r) => Some(r),
            }
        }
        s => s,
    };

    let username = match config.get_str("github.username") {
        Err(error) => {
            println!("GitHub username not provided, use `git config --global --add \
                      github.username USERNAME` to set your username: {}",
                     error);
            process::exit(1);
        }
        Ok(name) => name,
    };

    let repo = match init_storage(url.unwrap()) {
        Err(error) => {
            println!("failed to open repository: {}", error);
            process::exit(1);
        }
        Ok(r) => r,
    };

    if let Err(error) = init_sync(username, &repo) {
        println!("failed the initial sync: {}", error);
        process::exit(1);
    }

    let interval = value_t!(matches.value_of("interval"), u64).unwrap_or(1800);
    let tick = schedule_recv::periodic(Duration::from_secs(interval));
    loop {
        tick.recv().unwrap();

        let state = sync_repo(username, &repo);
        if state.is_err() {
            println!("failed the sync repository: {}",
                     state.err().unwrap().message());
            process::exit(1);
        }
    }
}

fn init_storage(url: &str) -> Result<Repository, Error> {
    let home = env::home_dir().unwrap();
    let storage = home.join(STORE_NAME);

    if storage.exists() {
        if storage.is_file() {
            println!("{} is a file!", storage.to_string_lossy());
            process::exit(1);
        }

        return Repository::open(storage.as_path());
    }
    RepoBuilder::new().bare(false).clone(url, storage.as_path())
}

fn init_sync(username: &str, repo: &Repository) -> Result<(), Error> {
    reset_master(username, repo)?;
    sync_repo(username, repo)
}

fn sync_repo(username: &str, repo: &Repository) -> Result<(), Error> {
    sync_files(repo.workdir().unwrap());

    let statuses = repo.statuses(None)?;
    if statuses.len() > 0 {
        return commit_updates(&repo);
    }

    push_master(username, repo)
}

fn commit_updates(repo: &Repository) -> Result<(), Error> {
    let head_commit = repo.find_commit(repo.refname_to_id("HEAD")?)?;

    let mut index = repo.index()?;
    index.add_all(&["**/*"], git2::ADD_DEFAULT, None)?;
    index.write()?;

    let oid = index.write_tree()?;
    let tree = repo.find_tree(oid)?;

    let author = repo.signature()?;
    repo.commit(Some("HEAD"),
                &author,
                &author,
                "Update files",
                &tree,
                &[&head_commit])?;

    Ok(())
}


fn reset_master(username: &str, repo: &Repository) -> Result<(), Error> {
    let mut remote = repo.find_remote("origin")?;
    let mut cb = RemoteCallbacks::new();
    cb.credentials(|url, _, _| git_credentials(username, url));

    let mut fo = FetchOptions::new();
    fo.remote_callbacks(cb);
    remote.fetch(&[], Some(&mut fo), None)?;

    let reference = "refs/remotes/origin/master";
    let oid = repo.refname_to_id(reference)?;
    let object = repo.find_object(oid, None)?;
    repo.reset(&object, git2::ResetType::Hard, None)
}

fn push_master(username: &str, repo: &Repository) -> Result<(), Error> {
    let mut remote = repo.find_remote("origin")?;
    let mut cb = RemoteCallbacks::new();
    cb.credentials(|url, _, _| git_credentials(username, url));

    let mut po = PushOptions::new();
    po.remote_callbacks(cb);

    remote.push(&["refs/remotes/origin/master"], Some(&mut po))
}

fn sync_files(workdir: &std::path::Path) {
    let walker = WalkDir::new(workdir)
        .into_iter()
        .filter_entry(|e| !is_git_object(e));

    for entry in walker {
        let entry = entry.unwrap();
        if entry.file_type().is_file() {
            let p = PathBuf::from(entry.path());
            let name = p.strip_prefix(workdir).unwrap();

            if let Err(error) = sync_path(entry.path(), name) {
                println!("[WARNING] Unable to sync file {}: {}",
                         name.display(),
                         error);
            }
        }
    }
}

fn sync_path(full_path: &std::path::Path,
             base_path: &std::path::Path)
             -> Result<(), std::io::Error> {
    let home = env::home_dir().unwrap();
    let sync_path = home.join(base_path);

    if sync_path.exists() {
        match fs::copy(home.join(sync_path).as_path(), full_path) {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    } else {
        fs::remove_file(full_path)
    }
}

fn is_git_object(entry: &DirEntry) -> bool {
    entry.file_name()
        .to_str()
        .map(|s| s.starts_with(".git"))
        .unwrap_or(false)
}

fn git_credentials(username: &str, url: &str) -> Result<git2::Cred, Error> {
    if url.starts_with("https://") {
        match SecretService::new(EncryptionType::Dh) {
            Err(error) => {
                Err(Error::from_str(format!("Unable to connect with the secret service: {}",
                                            error)
                    .as_str()))
            }
            Ok(ss) => {
                match ss.search_items(vec![("github", "access_token")]) {
                    Err(_) => {
                        Err(Error::from_str("GitHub credentials are not in the store, use `flake \
                                         auth` to set them up"))
                    }
                    Ok(items) => {
                        let item = items.get(0).unwrap();
                        match item.get_secret() {
                            Err(_) => {
                                Err(Error::from_str("Missing access token, use `flake auth` to \
                                                     set it up"))
                            }
                            Ok(bytes) => {
                                let token = String::from_utf8(bytes).unwrap();
                                git2::Cred::userpass_plaintext(username, token.as_str())
                            }
                        }
                    }
                }
            }
        }
    } else {
        let home = env::home_dir().unwrap();
        let private_key = home.join(".ssh/id_rsa");
        let public_key = home.join(".ssh/id_rsa.pub");

        git2::Cred::ssh_key("git",
                            Some(public_key.as_path()),
                            private_key.as_path(),
                            None)
    }
}
