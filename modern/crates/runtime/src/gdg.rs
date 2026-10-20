//! Generation file naming: the local counterpart of the mainframe's generation data groups.
//!
//! On the mainframe, `IDCAMS` defines a generation data group base for each repeated output:
//! `TRANREPT`, `SYSTRAN` and the backup bases in `DEFGDGB.jcl`, and `DALYREJS` in `DALYREJS.jcl`,
//! each with `LIMIT(5) SCRATCH`. Every run then catalogues a new generation through a `(+1)` DD
//! with `DISP=(NEW,CATLG,DELETE)`: `DALYREJS(+1)` in `POSTTRAN` (430-byte records), `SYSTRAN(+1)`
//! in `INTCALC` (350-byte records) and `TRANREPT(+1)` in `TRANREPT` (133-byte records). The online
//! report request (`CR00`, program `CORPT00C`) submits job `TRNRPT00` through the `JOBS` transient
//! data queue, and its job decks take the same naming here.
//!
//! A base becomes a local directory, and generation `nnnn` of base `NAME` becomes the file
//! `NAME.Gnnnn` in it. [`next_generation`] returns the `(+1)` path: one above the highest
//! generation present in the directory, or `G0001` when there is none.
//!
//! Caller contract:
//! - Create the returned file with `std::fs::OpenOptions::new().write(true).create_new(true)`.
//!   A `NEW` data set never replaces an existing one, and `create_new` keeps that property: when
//!   two runs race for the same generation, the second fails instead of overwriting the first.
//! - Print the path written to standard output, one per line, so the operator learns the
//!   generation number without listing the directory.
//!
//! Old generations are never removed. `LIMIT(5) SCRATCH` has no counterpart, because no part of
//! the `CardDemo` replacement asks for pruning. Every path is on the local filesystem; no object
//! storage is involved.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Highest generation number the four-digit `Gnnnn` suffix can hold.
const MAX_GENERATION: u16 = 9999;

/// Returns the path of generation `(+1)` of base `name` in `dir`, `<dir>/<name>.G<nnnn>`, with
/// `nnnn` one above the highest existing generation, or `0001` when `dir` holds none.
///
/// `dir` and its missing parents are created first, as cataloguing a new generation never needs
/// the base to be prepared by hand. The returned file itself is not created: the caller creates
/// it exclusively, as the module documentation describes.
///
/// A directory entry is a generation only when its name is exactly `<name>.G` followed by four
/// ASCII digits, compared case-sensitively. Entries of every type count (files, directories and
/// symbolic links, dangling ones included); every other name is ignored, as is any name that is
/// not valid UTF-8.
///
/// # Errors
///
/// - [`io::ErrorKind::InvalidInput`] when `name` is empty or contains `/`, `\` or a NUL
///   character, since the generation file would then leave `dir` or have no valid file name.
///   `dir` is left untouched in that case.
/// - Any error from creating `dir`, reading it, or reading one of its entries, for example when
///   `dir` names an existing regular file.
/// - An error of kind [`io::ErrorKind::Other`] when generation 9999 of `name` already exists. The
///   number never wraps back to `G0001`, which would overwrite the oldest generation.
pub fn next_generation(dir: &Path, name: &str) -> io::Result<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "generation base name must be non-empty and contain no path separator or NUL character",
        ));
    }

    fs::create_dir_all(dir)?;

    let prefix = format!("{name}.G");
    let mut highest: Option<u16> = None;
    for entry in fs::read_dir(dir)? {
        let file_name = entry?.file_name();
        let generation = file_name
            .to_str()
            .and_then(|text| parse_generation(text, &prefix));
        highest = highest.max(generation);
    }

    let next = match highest {
        None => 1,
        Some(generation) => generation
            .checked_add(1)
            .filter(|next| *next <= MAX_GENERATION)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "generation limit {MAX_GENERATION} reached for {name}"
                ))
            })?,
    };

    Ok(dir.join(format!("{name}.G{next:04}")))
}

/// Generation number of `file_name` when it is `prefix` followed by exactly four ASCII digits.
fn parse_generation(file_name: &str, prefix: &str) -> Option<u16> {
    let digits = file_name.strip_prefix(prefix)?;
    if digits.len() == 4 && digits.bytes().all(|byte| byte.is_ascii_digit()) {
        digits.parse::<u16>().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::next_generation;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};

    /// A directory private to one test of this process, absent at creation and removed on drop,
    /// so a failing assertion leaves nothing behind either.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(test_name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("carddemo-gdg-{}-{test_name}", std::process::id()));
            remove_if_present(&path);
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn create_file(&self, file_name: &str) {
            fs::create_dir_all(self.path()).expect("create scratch directory");
            fs::write(self.path().join(file_name), b"").expect("create scratch file");
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            remove_if_present(&self.0);
        }
    }

    fn remove_if_present(path: &Path) {
        match fs::remove_dir_all(path) {
            Err(error) if error.kind() != ErrorKind::NotFound => {
                // A second panic while a failed test unwinds would abort the whole test binary.
                if std::thread::panicking() {
                    eprintln!("cannot remove {}: {error}", path.display());
                } else {
                    panic!("cannot remove {}: {error}", path.display());
                }
            }
            _ => {}
        }
    }

    #[test]
    fn absent_directory_is_created_and_yields_first_generation() {
        let scratch = ScratchDir::new("absent");
        let dir = scratch.path().join("var").join("dalyrejs");

        let first = next_generation(&dir, "DALYREJS").expect("first generation");
        assert_eq!(first, dir.join("DALYREJS.G0001"));
        assert!(dir.is_dir());
        assert!(!first.exists());

        // The caller contract: exclusive creation advances the next call and never overwrites.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&first)
            .expect("create first generation");
        assert_eq!(
            next_generation(&dir, "DALYREJS").expect("second generation"),
            dir.join("DALYREJS.G0002")
        );
        let again = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&first)
            .expect_err("an existing generation is never reopened");
        assert_eq!(again.kind(), ErrorKind::AlreadyExists);
    }

    #[test]
    fn highest_generation_of_any_entry_type_wins() {
        let scratch = ScratchDir::new("highest");
        scratch.create_file("DALYREJS.G0001");
        scratch.create_file("DALYREJS.G0003");
        fs::create_dir(scratch.path().join("DALYREJS.G0004")).expect("create directory entry");

        assert_eq!(
            next_generation(scratch.path(), "DALYREJS").expect("next generation"),
            scratch.path().join("DALYREJS.G0005")
        );
    }

    #[test]
    fn names_that_are_not_generations_of_the_base_are_ignored() {
        let scratch = ScratchDir::new("ignored");
        for file_name in [
            "OTHER.G0009",
            "DALYREJS.G12",
            "DALYREJS.G00010",
            "DALYREJS.G0002.tmp",
            "DALYREJS.Gabcd",
            "DALYREJS.G+123",
            "dalyrejs.G0005",
            "DALYREJS.g0003",
        ] {
            scratch.create_file(file_name);
        }

        assert_eq!(
            next_generation(scratch.path(), "DALYREJS").expect("next generation"),
            scratch.path().join("DALYREJS.G0001")
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_links_count_and_non_utf8_names_are_ignored() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let scratch = ScratchDir::new("unix-entries");
        fs::create_dir_all(scratch.path()).expect("create scratch directory");
        std::os::unix::fs::symlink(
            scratch.path().join("missing-target"),
            scratch.path().join("DALYREJS.G0007"),
        )
        .expect("create dangling link");
        fs::write(
            scratch
                .path()
                .join(OsStr::from_bytes(b"DALYREJS.G\xff\xfe12")),
            b"",
        )
        .expect("create non-UTF-8 entry");

        assert_eq!(
            next_generation(scratch.path(), "DALYREJS").expect("next generation"),
            scratch.path().join("DALYREJS.G0008")
        );
    }

    #[test]
    fn generation_limit_is_an_error_and_never_wraps() {
        let scratch = ScratchDir::new("limit");
        scratch.create_file("SYSTRAN.G9999");

        let error = next_generation(scratch.path(), "SYSTRAN").expect_err("limit reached");
        assert_eq!(error.kind(), ErrorKind::Other);
        assert!(error.to_string().contains("9999"));
    }

    #[test]
    fn invalid_base_names_are_rejected_before_touching_the_directory() {
        let scratch = ScratchDir::new("invalid-names");
        for name in ["", "a/b", "a\\b", "a\0b"] {
            let error = next_generation(scratch.path(), name).expect_err("invalid name");
            assert_eq!(error.kind(), ErrorKind::InvalidInput, "name {name:?}");
        }
        assert!(!scratch.path().exists());
    }

    #[test]
    fn regular_file_in_place_of_the_directory_is_an_error() {
        let scratch = ScratchDir::new("file-as-dir");
        scratch.create_file("not-a-directory");

        assert!(next_generation(&scratch.path().join("not-a-directory"), "DALYREJS").is_err());
    }

    #[test]
    fn bases_in_one_directory_are_independent() {
        let scratch = ScratchDir::new("independent");
        scratch.create_file("TRANREPT.G0002");

        assert_eq!(
            next_generation(scratch.path(), "TRNRPT00").expect("job deck generation"),
            scratch.path().join("TRNRPT00.G0001")
        );
        assert_eq!(
            next_generation(scratch.path(), "TRANREPT").expect("report generation"),
            scratch.path().join("TRANREPT.G0003")
        );
    }
}
