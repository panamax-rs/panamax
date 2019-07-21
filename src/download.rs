use std::path::{Path, PathBuf};
use std::{io, fs};
use std::fs::File;
use sha2::{Digest, Sha256};
use std::io::{Read, Write, ErrorKind};

// General download strategy:
// 1: Download the sha256 file (or acquire the needed sha256 for a file)
// 2: Download the file to <name>.part and check sha256 as it downloads
// 3: Only when file is fully download and sha256 verified, move file to <name>
// If the <name> file already exists, don't bother downloading it again
// If downloading fails (sha256 doesn't match), retry downloading up to 5 times.
// If retries run out, keep note of the failure somewhere.

quick_error! {
    #[derive(Debug)]
    pub enum DownloadError {
        Io(err: io::Error) {
            from()
        }
        Download(err: reqwest::Error) {
            from()
        }
    }
}

/// Download a URL and return it as a string.
pub fn download_string(from: &str) -> Result<String, DownloadError> {
    Ok(reqwest::get(from)?.text()?)
}

/// Write a string to a file, creating directories if needed.
pub fn write_file_create_dir(path: &Path, contents: &str) -> Result<(), DownloadError> {
    let mut res = fs::write(path, contents);

    if let Err(e) = &res {
        if e.kind() == io::ErrorKind::NotFound {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            res = fs::write(path, contents);
        }
    }

    Ok(res?)
}

/// Create a file, creating directories if needed.
pub fn create_file_create_dir(path: &Path) -> Result<File, DownloadError> {
    let mut file_res = File::create(path);
    if let Err(e) = &file_res {
        if e.kind() == io::ErrorKind::NotFound {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            file_res = File::create(path);
        }
    }

    Ok(file_res?)
}

/// Download a file to a path, creating directories if needed.
pub fn download_and_create_dir(from: &str, to: &Path) -> Result<(), DownloadError> {
    let mut http_res = reqwest::get(from)?;

    let mut f = create_file_create_dir(to)?;

    http_res.copy_to(&mut f)?;

    Ok(())
}

/// Get the (lowercase hex) sha256 hash of a file.
pub fn file_sha256(path: &Path) -> Result<String, DownloadError> {
    let mut file = File::open(path)?;
    let mut sha256 = Sha256::new();
    fast_copy(&mut file, &mut sha256)?;
    Ok(format!("{:x}", sha256.result()))
}

/// If a file doesn't match a provided sha256, download a url to a path.
pub fn download_with_sha256_str_verify(url: &str, path: &Path, remote_sha256: &str) -> Result<(), DownloadError> {
    let do_download = if let Ok(local_file_sha256) = file_sha256(path) {
        remote_sha256 != local_file_sha256
    } else {
        true
    };

    if do_download {
        download_and_create_dir(url, path)?;
    };

    Ok(())
}

/// If an accompanying .sha256 file doesn't match or exist, download a url to a path.
pub fn download_with_sha256_verify(url: &str, path: &Path) -> Result<(), DownloadError> {
    let sha256_url = format!("{}.sha256", url);
    let sha256_path = {
        let mut new_path = path.as_os_str().to_os_string();
        new_path.push(".sha256");
        PathBuf::from(new_path)
    };

    let remote_sha256 = download_string(&sha256_url)?;

    let do_download = if let Ok(local_sha256) = fs::read_to_string(&sha256_path) {
        if local_sha256 == remote_sha256 {
            if let Ok(local_file_sha256) = file_sha256(&path) {
                remote_sha256[..local_file_sha256.len()] != local_file_sha256 // Download if sha256 doesn't match
            } else {
                true // Local file doesn't exist or couldn't be read, so try to download
            }
        } else {
            true // Local sha256 file doesn't match, so download
        }
    } else {
        true // Local sha256 file not found, so download
    };

    if do_download {
        write_file_create_dir(&sha256_path, &remote_sha256)?;
        download_and_create_dir(url, path)?;
    }

    Ok(())
}

/// Clone of the io::copy code, but with the buffer size changed to 64k
pub fn fast_copy<R: ?Sized, W: ?Sized>(reader: &mut R, writer: &mut W) -> io::Result<u64>
    where R: Read, W: Write
{
    let mut buf: [u8; 65536] = [0; 65536];

    let mut written = 0;
    loop {
        let len = match reader.read(&mut buf) {
            Ok(0) => return Ok(written),
            Ok(len) => len,
            Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        writer.write_all(&buf[..len])?;
        written += len as u64;
    }
}
