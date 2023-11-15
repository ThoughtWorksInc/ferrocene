// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: The Ferrocene Developers

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::Error;
use crate::report::Reporter;
use crate::targets::Target;
use crate::utils::{find_binary_in_path, run_command};
use crate::Environment;

/// What kind of C compiler does a target require
#[derive(Debug)]
pub enum Linker {
    /// No C compiler required
    BundledLld,
    /// The system's native C compiler is required
    HostCC,
    /// Some kind of cross compiler, with one of the given target prefixes
    CrossCC(&'static [&'static str]),
}

/// Finds a system C compiler for each target and determines what flags should
/// be added when calling `rustc`.
///
/// These flags will include `-Clinker=/path/to/cc` and they will be added to
/// each target's rustflags field.
pub(crate) fn check_and_add_rustflags(
    reporter: &dyn Reporter,
    environment: &Environment,
    sysroot: &Path,
    targets: &mut [Target],
) -> Result<(), Error> {
    // Step 1. Check we have ld.lld available
    let _rust_lld_path = find_bundled_lld(reporter, sysroot)?;
    let lld_bin = find_bundled_lld_wrapper(reporter, sysroot)?;
    let lld_dir = lld_bin.parent().expect("ld.lld should be a in a directory");

    // Step 2. Check the C compiler works on each target that needs one
    'target_loop: for target in targets {
        let prefix_list: &[&str] = match target.linker {
            Linker::BundledLld => {
                reporter
                    .skipped(&format!("Target `{}` does not require a C compiler", target.triple));
                continue 'target_loop;
            }
            Linker::HostCC => &[""],
            Linker::CrossCC(list) => list,
        };
        for cc_prefix in prefix_list {
            for compiler_kind in ["cc", "gcc", "clang"] {
                let temp_dir = tempfile::tempdir().map_err(|error| {
                    Error::TemporaryCompilationDirectoryCreationFailed { error }
                })?;
                let compiler_name = format!("{cc_prefix}{compiler_kind}");
                let cc_result = check_system_compiler(
                    environment,
                    target.triple,
                    &compiler_name,
                    lld_dir,
                    temp_dir.path(),
                );
                match cc_result {
                    Ok((_path, args)) => {
                        reporter.success(&format!(
                            "Found C compiler `{}` for target `{}`",
                            compiler_name, target.triple
                        ));
                        let mut need_no_lto = false;
                        for arg in args {
                            if std::env::var("FST_PRINT_LINKER_ARGS").is_ok() {
                                reporter
                                    .note(&format!("`{compiler_name}` provides argument {arg:?}"));
                            }
                            if arg.contains("liblto_plugin") {
                                need_no_lto = true;
                            }
                        }
                        target.rustflags.push(format!("-Clinker={compiler_name}"));
                        if need_no_lto {
                            target.rustflags.push(format!("-Clink-arg=-fno-lto"));
                        }
                        continue 'target_loop;
                    }
                    Err(e) => {
                        // Try again until we run out of compilers
                        if std::env::var("FST_PRINT_DETAILED_ERRORS").is_ok() {
                            reporter.note(&format!("`{compiler_name}` failed with {e}"));
                        }
                    }
                }
            }
        }
        return Err(Error::SuitableCCompilerNotFound { target: target.triple.to_owned() });
    }

    Ok(())
}

/// Check if the given system C compiler works.
///
/// We are given a path to the real `ld.lld`, but we also test it with a fake
/// `ld.lld` as well.
///
/// We are also given a path to a fresh temporary directory we can put source
/// code in.
///
/// Returns the path to the C compiler, and a list of arguments that the C
/// compiler gives to the linker.
fn check_system_compiler(
    environment: &Environment,
    target: &str,
    compiler_name: &str,
    lld_dir: &Path,
    temp_dir: &Path,
) -> Result<(PathBuf, Vec<String>), Error> {
    let cc_path = find_binary_in_path(environment, &compiler_name)
        .map_err(|error| Error::CCompilerNotFound { name: compiler_name.to_owned(), error })?;

    // Part 1. Check with the real ld.lld - can we make a binary?

    cross_compile_test_program(&cc_path, lld_dir, temp_dir)?;

    // Part 2. Make a fake linker, and get GCC to try and use it. What arguments
    // does it give our fake linker?

    let args_file = make_fake_linker(temp_dir)?;

    let linker_args = check_compiler_linker_args(target, &cc_path, temp_dir, &args_file)?;

    Ok((cc_path, linker_args))
}

/// Check if the given system C compiler works.
///
/// We are given a path to the real `ld.lld`.
///
/// We are also given a path to a fresh temporary directory we can put source
/// code in. We don't execute the program that we build as this might be a
/// cross-compiled target.
fn cross_compile_test_program(
    cc_path: &Path,
    lld_dir: &Path,
    temp_dir: &Path,
) -> Result<(), Error> {
    // We need some C source code,
    let c_source = r#"int main(void) { return 0; }"#;

    // We need a temp directory we can save the output file to
    let source_file = temp_dir.join("input.c");
    let object_file = temp_dir.join("output.bin");
    std::fs::write(&source_file, c_source.as_bytes()).map_err(|error| {
        Error::WritingSampleProgramFailed {
            name: "input.c".to_owned(),
            dest: source_file.clone(),
            error,
        }
    })?;

    // We need to call the C compiler, telling it to use ld.lld and telling it where to find ld.lld
    let args: Vec<OsString> = vec![
        "-fuse-ld=lld".into(),
        "-B".into(),
        lld_dir.as_os_str().to_owned(),
        source_file.as_os_str().to_owned(),
        "-o".into(),
        object_file.as_os_str().to_owned(),
    ];
    let mut cc_child = Command::new(cc_path);
    cc_child.args(&args);

    let _output = run_command(&mut cc_child).map_err(|error| {
        let cc_name: &str =
            cc_path.file_name().and_then(|p| p.to_str()).unwrap_or("<non UTF-8 compiler name>");
        Error::SampleProgramCompilationFailed { name: cc_name.to_string(), error }
    })?;

    Ok(())
}

/// Compile a fake linker using the host's C compiler
///
/// Returns the file that the fake linker will write its args to.
///
/// The linker itself is called `<temp_dir>/ld.lld`.
fn make_fake_linker(temp_dir: &Path) -> Result<PathBuf, Error> {
    const C_SOURCE: &[u8] = br#"
    #include <stdio.h>

    int main(int argc, char** argv) {
        FILE* f = fopen(""#;

    const C_SOURCE2: &[u8] = br#"", "w");
        if (!f) {
            return 1;
        }

        for (int arg = 0; arg < argc; arg++) {
            fprintf(f, "%s", argv[arg]);
            fprintf(f, "\r\n");
        }

        fclose(f);

        return 0;
    }
    "#;

    let args_file = temp_dir.join("_fst_args_capture");

    // Concatentation, using byte strings
    let mut c_source = C_SOURCE.to_owned();
    c_source.extend(args_file.as_os_str().as_encoded_bytes());
    c_source.extend(C_SOURCE2);

    let source_file = temp_dir.join("ldlld.c");
    let object_file = temp_dir.join("ld.lld");
    std::fs::write(&source_file, &c_source).map_err(|error| Error::WritingSampleProgramFailed {
        name: "ldlld.c".to_owned(),
        dest: source_file.clone(),
        error,
    })?;

    // Compile our sample program
    let args: Vec<OsString> =
        vec![source_file.as_os_str().to_owned(), "-o".into(), object_file.as_os_str().to_owned()];
    // Always use the host compiler for this build
    let mut cc_child = Command::new("cc");
    cc_child.args(&args);

    let _output = run_command(&mut cc_child)
        .map_err(|error| Error::SampleProgramCompilationFailed { name: "cc".to_string(), error })?;

    Ok(args_file)
}

/// Use a fake linker to check the C compiler arguments to the linker
///
/// The fake linker should be at `<temp_dir>/ld.lld` and it should write its
/// arguments to the path given by `args_file_path`.
///
/// Returns a list of arguments given to the fake linker.
fn check_compiler_linker_args(
    target: &str,
    cc_path: &Path,
    temp_dir: &Path,
    args_file_path: &Path,
) -> Result<Vec<String>, Error> {
    // Ensure this file doesn't already exist
    let _ = std::fs::remove_file(args_file_path);

    // compile a sample C program, but using our fake linker
    cross_compile_test_program(cc_path, temp_dir, temp_dir)?;

    // see what the fake linker wrote
    let Ok(args_file) = std::fs::read(args_file_path) else {
        return Err(Error::LinkerArgsError { target: target.to_owned() });
    };
    let Ok(args_str) = std::str::from_utf8(&args_file) else {
        return Err(Error::LinkerArgsError { target: target.to_owned() });
    };

    // parse the file
    let args: Vec<String> = args_str.lines().map(|s| s.to_owned()).collect();

    // an empty file would be bad
    if args.is_empty() {
        return Err(Error::LinkerArgsError { target: target.to_owned() });
    };

    Ok(args)
}

/// Look for the bundled `rust-lld` program in the given sysroot.
fn find_bundled_lld(reporter: &dyn Reporter, sysroot: &Path) -> Result<PathBuf, Error> {
    let path = sysroot
        .join("lib")
        .join("rustlib")
        .join(env!("SELFTEST_TARGET"))
        .join("bin")
        .join("rust-lld");

    if path.is_file() {
        reporter.success(&format!("bundled linker detected"));
        Ok(path)
    } else {
        Err(Error::BundledLinkerMissing)
    }
}

/// Look for the bundled `ld.lld` linker wrapper program in the given sysroot.
fn find_bundled_lld_wrapper(reporter: &dyn Reporter, sysroot: &Path) -> Result<PathBuf, Error> {
    let path = sysroot
        .join("lib")
        .join("rustlib")
        .join(env!("SELFTEST_TARGET"))
        .join("bin")
        .join("gcc-ld")
        .join("ld.lld");

    if path.is_file() {
        reporter.success(&format!("bundled linker-wrapper detected"));
        Ok(path)
    } else {
        Err(Error::BundledLinkerMissing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{error::FindBinaryInPathError, test_utils::TestUtils};

    #[test]
    fn test_find_bundled_lld() {
        let utils = TestUtils::new();
        utils.bin("rust-lld").for_target(env!("SELFTEST_TARGET")).create();

        find_bundled_lld(utils.reporter(), utils.sysroot()).unwrap();
        utils.assert_report_success("bundled linker detected");
    }

    #[test]
    fn test_find_bundled_lld_missing() {
        let utils = TestUtils::new();

        match find_bundled_lld(utils.reporter(), utils.sysroot()) {
            Err(Error::BundledLinkerMissing) => {
                // Ok
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_find_bundled_lld_wrapper() {
        let utils = TestUtils::new();
        utils.bin("gcc-ld/ld.lld").for_target(env!("SELFTEST_TARGET")).create();

        find_bundled_lld_wrapper(utils.reporter(), utils.sysroot()).unwrap();
        utils.assert_report_success("bundled linker-wrapper detected");
    }

    #[test]
    fn test_find_bundled_lld_wrapper_missing() {
        let utils = TestUtils::new();

        match find_bundled_lld_wrapper(utils.reporter(), utils.sysroot()) {
            Err(Error::BundledLinkerMissing) => {
                // Ok
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_c_compiler() {
        let utils = TestUtils::new();
        let temp_dir = tempfile::tempdir().unwrap();
        let expected_input = temp_dir.path().join("input.c");
        let expected_output = temp_dir.path().join("output.bin");

        let test_cc = utils
            .bin("custom-cc")
            .expected_args(&[
                "-fuse-ld=lld",
                "-B",
                "/some/fake/lld/path",
                expected_input.to_str().unwrap(),
                "-o",
                expected_output.to_str().unwrap(),
            ])
            .create();

        // Having constructed a fake C compiler, we should be able to call it
        cross_compile_test_program(&test_cc, Path::new("/some/fake/lld/path"), temp_dir.path())
            .expect("Working C compiler");
    }

    #[test]
    fn test_c_compiler_missing() {
        let utils = TestUtils::new();
        let temp_dir = tempfile::tempdir().expect("making temp dir");

        // Having constructed a fake C compiler, we should be able to call it
        let result = check_system_compiler(
            utils.env(),
            "some-test-target",
            "missing-cc",
            Path::new("/some/fake/lld/path"),
            temp_dir.path(),
        );
        match result {
            Ok(_) => {
                panic!("Should not have found a C compiler");
            }
            Err(Error::CCompilerNotFound {
                name,
                error: FindBinaryInPathError::MissingBinary { .. },
            }) => {
                assert_eq!(&name, "missing-cc");
            }
            _ => {
                panic!("Unexpected error");
            }
        }
    }

    #[test]
    fn test_make_fake_linker() {
        let temp_dir = tempfile::tempdir().expect("making temp dir");
        // make a fake linker
        let args_path = make_fake_linker(temp_dir.path()).unwrap();
        let args: &[OsString] = &["-arg1".into(), "-arg2".into(), "-arg3".into()];
        let lld_path = temp_dir.path().join("ld.lld");

        // Run the fake linker
        let mut lld_child = Command::new(&lld_path);
        lld_child.args(args);
        let _output = run_command(&mut lld_child).unwrap();

        // Did the fake linker write all the command-line args to the file (including its own name)
        let read_args = std::fs::read_to_string(args_path).unwrap();
        let mut lines = read_args.lines();
        assert_eq!(lines.next(), Some(lld_path.to_str().unwrap()));
        for arg in args {
            assert_eq!(lines.next(), Some(arg.to_str().unwrap()));
        }
        assert!(lines.next().is_none());
    }
}
