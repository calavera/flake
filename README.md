# Introduction

Flake is a daemon that monitors your computer's configuration files and stores them in a git repository.

Flake is released under the [MIT License](LICENSE).
Please make sure you understand its [implications and guarantees](https://writing.kemitchell.com/2016/09/21/MIT-License-Line-by-Line.html).

*Disclaimer:* **This project only works on Linux, and it probably has many bugs**

# Installation

You need to clone this repository and build the source, because I'm a noob in distributing Rust packages.

```
git clone https://github.com/calavera/flake && cd flake && cargo install
```

# Usage

Please, feel free to make it work in other platforms.

1- Add your dotfiles url and github username to your global git configuration, like this:

```
git config --global --add github.username calavera
git config --global --add github.dotfiles https://github.com/calavera/dotfiles
```

2- Add an authentication token to the secrets storage, like this:

```
flake auth YOUR_TOKEN
```

3- Launch the sync process:

```
flake sync
```

This will make flake to run in the foreground and check for changes on your dotfiles every 30 minutes.
You can use systemD or your less favourite init system to make it run as a daemon in the background.

When flake detects changes in your files, it will push them to your remote repository grouped in a single commit.
