# Panamax

Panamax is a tool to mirror the `rustup` and crates.io repositories, for offline usage of Rust and `cargo`.

## Installation

Panamax is itself available on crates.io, and can be installed via:

```
$ cargo install panamax
```

Alternatively, you can clone this repository and `cargo build` or `cargo run` within it.

## Usage


### Init

In Panamax, mirrors consist of self-contained directories. To create a mirror directory `my-mirror`:

```
$ panamax init my-mirror
Successfully created mirror base at `my-mirror`.
Make any desired changes to my-mirror/mirror.toml, then run panamax sync my-mirror.
```

There will now be a `my-mirror` directory in your current directory.

### Modify mirror.toml

Within the directory, you'll find a `mirror.toml` file. This file contains the full configuration of the mirror, and while it has sane defaults, you should ensure the values are set to what you want.

**This is especially important for the contact information, which will help prevent you from getting your IP address banned from crates.io.**

The other important parameter to set is the `base_url` within the `[crates]` section. After `cargo` fetches the index, it will try to use this URL to actually download the crates. It's important this value is accurate, or `cargo` may not work with the mirror.

You can modify `mirror.toml` at any point in time, even after the mirror is synchronized.

### Sync

Once you have made the changes to `mirror.toml`, it is time to synchronize your mirror!

```
$ panamax sync my-mirror
Syncing Rustup repositories...
[1/5] Syncing rustup-init files... ██████████████████████████████████████████████████████████████ 27/27 [00:00:06]
[2/5] Syncing latest stable...     ████████████████████████████████████████████████████████████ 602/602 [00:09:02]
[3/5] Syncing latest beta...       ████████████████████████████████████████████████████████████ 524/524 [00:07:29]
[4/5] Syncing latest nightly...    ████████████████████████████████████████████████████████████ 546/546 [00:08:56]
[5/5] Cleaning old files...        ████████████████████████████████████████████████████████████ 546/546 [00:00:00]
Syncing Rustup repositories complete!
Syncing Crates repositories...
[1/3] Syncing crates.io-index...   ██████████████████████████████████████████████████████████ 1615/1615 [00:00:02]
[2/3] Syncing crates files...      ██████████████████████████████████████████████████████████ 6357/6357 [00:00:05]
[3/3] Merging crates.io-index...
Syncing Crates repositories complete!
Sync complete.
```

Once this is step completes (without download errors), you will now have a full, synchronized copy of all the files needed to use `rustup` and `cargo` to their full potential!

This directory can now be copied to a USB or rsync'd somewhere else, or even used in place - perfect for long plane trips!

Additionally, this mirror can continually by synchronized in the future - one recommendation is to run this command in a cronjob once each night, to keep the mirror reasonably up to date.

## Server

Panamax grabs the files needed to make a full mirror, however once the mirror directory is at its destination, it needs to be hosted as a server. Panamax doesn't provide this, however it should be fairly simple to host a mirror - everything can be accessed via HTTP, with the exception of the `crates.io-index` which uses git.

A sample `nginx` configuration file, `mirror.nginx.conf' has been provided in the repository which will handle hosting a mirror server. Use this in the `sites-available` nginx directory, or copy it into `nginx.conf`.

* TODO: create example nginx file *

## Configuring `rustup` and `cargo`

Once you have a mirror server set up and running, it's time to tell your Rust components to use it.

### Setting environment variables

In order to ensure `rustup` knows where to look for the Rust components, we need to set some environment variables. Assuming the mirror is hosted at http://panamax.internal/:

```
export RUSTUP_DIST_SERVER=http://panamax.internal
export RUSTUP_UPDATE_ROOT=http://panamax.internal/rustup
```

These need to be set whenever `rustup` is used, so these should be added to your `.bashrc` file (or equivalent).

### Installing `rustup`

If you already have `rustup` installed, this step isn't necessary, however if you don't have access to https://rustup.rs, the mirror also contains the `rustup-init` files needed to install `rustup`.

Assuming the mirror is hosted at http://panamax.internal/, you will find the `rustup-init` files at http://panamax.internal/rustup/dist/. The `rustup-init` file you want depends on your architecture. Assuming you're running desktop Linux on a 64-bit machine:

```
wget http://panamax.internal/rustup/dist/x86_64-unknown-linux-gnu/rustup-init
chmod +x rustup-init
./rustup-init
```

This will let you install `rustup` the similarly following the steps from https://rustup.rs. This will also let you use `rustup` to keep your Rust installation updated in the future.

### Configuring `cargo`

`Cargo` also needs to be configured to point to the mirror. This can be done by adding the following lines to `~/.cargo/config` (creating the file if it doesn't exist):

```
[source.my-mirror]
registry = "http://panamax.internal/crates.io-index"
[source.crates-io]
replace-with = "my-mirror"
```

`Cargo` should now be pointing to the correct location to use the mirror.

### Testing configuration

You've now set up a Rust mirror! In order to make sure everything is set up properly, you can run a simple test:

```
$ cargo install ripgrep
```

This will install the grep-like `rg` tool (which is a great tool - props to burntsushi!). If `cargo` successfully downloads and builds everything, you have yourself a working mirror. Congratulations!


## License

Licensed under the terms of the MIT license and the Apache License (Version 2.0)

See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE) for details.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
