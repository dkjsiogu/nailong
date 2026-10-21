use crate::error::{Error, Result, io};
use crate::http::{FileSpec, Http, sha1_file};
use crate::paths::Root;
use crate::progress::Sink;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Content-addressed store for files that are expensive to download or patch.
///
/// A blob is named by its SHA1.  Materialising it into a version/library path
/// uses a hard link when possible, so multiple instances can share one copy.
pub fn blob_dir(root: &Root) -> PathBuf {
    root.cache().join("blobs")
}

pub fn blob_path(root: &Root, sha1: &str) -> PathBuf {
    blob_dir(root).join(sha1.to_ascii_lowercase())
}

pub fn has(root: &Root, sha1: &str) -> bool {
    blob_path(root, sha1).is_file()
}

/// Hash `source` and put it in the blob store.  Returns the SHA1.
pub fn store(root: &Root, source: &Path) -> Result<String> {
    let sha1 = sha1_file(source)?;
    let dest = blob_path(root, &sha1);
    if dest.is_file() {
        return Ok(sha1);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let tmp = dest.with_extension(format!("part-{}", std::process::id()));
    if std::fs::hard_link(source, &tmp).is_err() {
        std::fs::copy(source, &tmp).map_err(|e| io(source, e))?;
    }
    std::fs::rename(&tmp, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(&dest, e)
    })?;
    Ok(sha1)
}

/// Link or copy a blob to `dest`.  Returns false if the blob is missing.
pub fn materialize(root: &Root, sha1: &str, dest: &Path) -> Result<bool> {
    let source = blob_path(root, sha1);
    if !source.is_file() {
        return Ok(false);
    }
    if dest.is_file() {
        let actual = sha1_file(dest)?;
        if actual.eq_ignore_ascii_case(sha1) {
            return Ok(true);
        }
        std::fs::remove_file(dest).map_err(|e| io(dest, e))?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let tmp = dest.with_extension(format!("part-{}", std::process::id()));
    if std::fs::hard_link(&source, &tmp).is_err() {
        std::fs::copy(&source, &tmp).map_err(|e| io(&source, e))?;
    }
    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(dest, e)
    })?;
    Ok(true)
}

/// Download a batch through the blob store.  Files with a known SHA1 are
/// fetched once into `blobs/<sha1>`, then hard-linked to their final paths.
/// Files without a hash keep the normal direct download path.
pub fn download_specs(
    root: &Root,
    http: &Http,
    specs: Vec<FileSpec>,
    label: &str,
    sink: &Sink,
) -> Result<usize> {
    let mut downloads = Vec::with_capacity(specs.len());
    let mut direct = Vec::new();
    let mut links: Vec<(String, PathBuf)> = Vec::new();
    let mut seen = HashSet::new();
    for mut spec in specs {
        let Some(sha1) = spec.sha1.clone() else {
            direct.push(spec);
            continue;
        };
        // Existing files should stay on the cheap size-check path; forcing
        // every installed library through the blob store would re-hash or
        // re-download hundreds of MB on a repair.
        let size_ok = spec.size.is_none_or(|size| {
            spec.dest
                .metadata()
                .map(|m| m.len() == size)
                .unwrap_or(false)
        });
        if spec.dest.is_file() && size_ok {
            direct.push(spec);
            continue;
        }
        let blob = blob_path(root, &sha1);
        links.push((sha1.clone(), spec.dest.clone()));
        if blob.is_file() {
            continue;
        }
        spec.dest = blob.clone();
        if seen.insert(blob) {
            downloads.push(spec);
        }
    }
    downloads.extend(direct);
    let count = downloads.len();
    if !downloads.is_empty() {
        http.download_many_with(downloads, label, sink)?;
    }
    for (sha1, dest) in links {
        if !materialize(root, &sha1, &dest)? {
            return Err(Error::Install(format!(
                "blob 缺失，无法落盘: {}",
                dest.display()
            )));
        }
    }
    Ok(count)
}

/// Verify a file against an expected SHA1, with a readable error.
pub fn verify(path: &Path, expected: &str) -> Result<()> {
    let actual = sha1_file(path)?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(Error::Checksum {
            path: path.to_path_buf(),
            expected: expected.to_string(),
            actual,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> Root {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Root::new(
            std::env::temp_dir().join(format!("nailong-blob-{tag}-{}-{n}", std::process::id())),
        )
    }

    #[test]
    fn stores_and_materializes_without_copying_content() {
        let root = temp_root("roundtrip");
        std::fs::create_dir_all(root.cache()).unwrap();
        let source = root.cache().join("source.bin");
        std::fs::write(&source, b"patched-client").unwrap();
        let sha1 = store(&root, &source).unwrap();
        assert!(has(&root, &sha1));
        let dest = root.libraries().join("client.jar");
        assert!(materialize(&root, &sha1, &dest).unwrap());
        assert_eq!(std::fs::read(&dest).unwrap(), b"patched-client");
        let _ = std::fs::remove_dir_all(root.dir());
    }
}
