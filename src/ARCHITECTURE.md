# Architecture

Panamax's main functionality is split up into two main components: Crates, and Rustup. Both of these pieces share the same functionality to download files.

Additionally when downloading files, a shared progress bar is used.

Finally, a mirror.toml file is used to configure everything.

# Starting Points

The two main commands, `init` and `sync`, are handled in the `init()` and `sync()` commands in mirror.rs.

## Main Components 

### Crates

The crates component is split up into two files: `crates_index.rs` for handling the crates.io-index git repository, and `crates.rs` for the crates files themselves.

### Rustup

The rustup component is covered in `rustup.rs`. This includes functionality to download the rustup-init files, as well as the libraries and components required for the various Rust versions.

## Shared Components

### Download

All details related to downloading (or more specifically, HTTP downloading) is covered in `download.rs`. This includes functionality such as retrying on failed downloads.

### Progress Bar

When a mirror is downloading or updating, a progress bar is displayed. This progress bar is used by all components and is made to be synchronized with multiple download threads.

All details related to this component is covered in `progress_bar.rs`.

### Mirror Configuration

All details related to configuration file management is handled in `mirror.rs`. Serde is used to parse the `mirror.toml` file, with the root being the `Mirror` struct.