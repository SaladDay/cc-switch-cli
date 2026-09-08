#[cfg(unix)]
mod links {
    use std::os::unix::fs::{symlink, PermissionsExt};

    use super::super::*;
    use crate::test_support::TestEnvGuard;

    const TARGETS: [McpConfigTarget; 5] = [
        McpConfigTarget::Claude,
        McpConfigTarget::Gemini,
        McpConfigTarget::Codex,
        McpConfigTarget::OpenCode,
        McpConfigTarget::Hermes,
    ];

    fn no_staging_files(parent: &Path) {
        assert!(fs::read_dir(parent).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".cc-switch-mcp-recovery-")));
    }

    #[test]
    fn creation_permissions_respect_existing_files_and_managed_privacy() {
        for target in TARGETS {
            for managed in [false, true] {
                for existing in [false, true] {
                    let temp = tempfile::tempdir().unwrap();
                    let _env = TestEnvGuard::isolated(temp.path());
                    let path = temp.path().join(if managed {
                        ".cc-switch/fixture.json"
                    } else {
                        "fixture.json"
                    });
                    if existing {
                        fs::create_dir_all(path.parent().unwrap()).unwrap();
                        fs::write(&path, "old").unwrap();
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
                    }
                    let mut file = NativeFile::observe(target, &path).unwrap();
                    file.set_creation_permissions(fs::Permissions::from_mode(0o444))
                        .unwrap();
                    file.publish("new").unwrap();
                    assert_eq!(
                        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                        if managed {
                            0o600
                        } else if existing {
                            0o640
                        } else {
                            0o444
                        }
                    );
                    file.rollback().unwrap();
                    assert_eq!(
                        fs::read_to_string(&path).ok().as_deref(),
                        existing.then_some("old")
                    );
                }
            }
        }
    }

    #[test]
    fn receipts_restore_absolute_relative_and_dangling_links() {
        for target in TARGETS {
            for relative in [false, true] {
                for original in [None, Some("old"), Some("new")] {
                    let temp = tempfile::tempdir().unwrap();
                    let _env = TestEnvGuard::isolated(temp.path());
                    let path = temp.path().join("config");
                    let source = temp.path().join("source");
                    if let Some(original) = original {
                        fs::write(&source, original).unwrap();
                        fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
                    }
                    let link = if relative {
                        Path::new("source")
                    } else {
                        &source
                    };
                    symlink(link, &path).unwrap();
                    let mut file = NativeFile::observe(target, &path).unwrap();
                    file.publish("new").unwrap();
                    assert!(!path.is_symlink());
                    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
                    file.rollback().unwrap();
                    assert_eq!(fs::read_link(&path).unwrap(), link);
                    assert_eq!(fs::read_to_string(&path).ok().as_deref(), original);
                    assert_eq!(fs::read_to_string(&source).ok().as_deref(), original);
                    if original.is_some() {
                        assert_eq!(
                            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                            0o640
                        );
                    }
                    drop(file);
                    no_staging_files(temp.path());
                }
            }
        }
    }

    #[test]
    fn uncertain_publication_recovers_links_before_and_after_replacement() {
        for target in TARGETS {
            for written in [false, true] {
                for original in [None, Some("old"), Some("new")] {
                    let temp = tempfile::tempdir().unwrap();
                    let _env = TestEnvGuard::isolated(temp.path());
                    let path = temp.path().join("config");
                    let source = temp.path().join("source");
                    if let Some(original) = original {
                        fs::write(&source, original).unwrap();
                    }
                    symlink("source", &path).unwrap();
                    let mut file = NativeFile::observe(target, &path).unwrap();
                    let mut first = true;
                    let error = with_exchange_hook(
                        Box::new(move |resource, replacement| {
                            if std::mem::take(&mut first) {
                                if written {
                                    resource.write(replacement.unwrap())?;
                                }
                                return Err(AppError::io(
                                    resource.path(),
                                    std::io::Error::other("fixture"),
                                ));
                            }
                            Ok(())
                        }),
                        || file.publish("new"),
                    )
                    .unwrap_err();
                    assert!(
                        !error.to_string().contains("recovery incomplete"),
                        "{error}"
                    );
                    assert_eq!(fs::read_link(&path).unwrap(), Path::new("source"));
                    assert_eq!(fs::read_to_string(&path).ok().as_deref(), original);
                    drop(file);
                    no_staging_files(temp.path());
                }
            }
        }
    }

    #[test]
    fn publication_detects_changed_links_and_referents() {
        for target in TARGETS {
            for retarget in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let _env = TestEnvGuard::isolated(temp.path());
                let path = temp.path().join("config");
                let source = temp.path().join("source");
                fs::write(&source, "old").unwrap();
                symlink("source", &path).unwrap();
                let mut file = NativeFile::observe(target, &path).unwrap();
                let changed = path.clone();
                with_exchange_hook(
                    Box::new(move |_, _| {
                        if retarget {
                            let replacement = changed.with_file_name("replacement");
                            fs::write(&replacement, "old").unwrap();
                            fs::remove_file(&changed).unwrap();
                            symlink("replacement", &changed).unwrap();
                        } else {
                            let staged = source.with_file_name("staged");
                            fs::write(&staged, "external").unwrap();
                            fs::rename(staged, &source).unwrap();
                        }
                        Ok(())
                    }),
                    || assert!(matches!(file.publish("new"), Err(AppError::Conflict(_)))),
                );
                assert_eq!(
                    fs::read_link(&path).unwrap(),
                    Path::new(if retarget { "replacement" } else { "source" })
                );
                assert_eq!(
                    fs::read_to_string(&path).unwrap(),
                    if retarget { "old" } else { "external" }
                );
                drop(file);
                no_staging_files(temp.path());
            }
        }
    }

    #[test]
    fn recovery_reports_external_changes_without_overwriting_them() {
        for target in TARGETS {
            for change in [
                "referent",
                "missing_referent",
                "link",
                "dangling_link",
                "original_bytes",
            ] {
                let temp = tempfile::tempdir().unwrap();
                let _env = TestEnvGuard::isolated(temp.path());
                let path = temp.path().join("config");
                let source = temp.path().join("source");
                fs::write(&source, "old").unwrap();
                symlink("source", &path).unwrap();
                let mut file = NativeFile::observe(target, &path).unwrap();
                file.publish("new").unwrap();
                match change {
                    "referent" => fs::write(&source, "external").unwrap(),
                    "missing_referent" => fs::remove_file(&source).unwrap(),
                    "original_bytes" => fs::write(&path, "old").unwrap(),
                    _ => {
                        if change == "link" {
                            fs::write(temp.path().join("external"), "old").unwrap();
                        }
                        fs::remove_file(&path).unwrap();
                        symlink("external", &path).unwrap();
                    }
                }
                let contents = fs::read(&path).ok();
                let link = fs::read_link(&path).ok();
                let referent = fs::read(&source).ok();
                assert!(file.rollback().is_err(), "{change}");
                assert_eq!(fs::read(&path).ok(), contents);
                assert_eq!(fs::read_link(&path).ok(), link);
                assert_eq!(fs::read(&source).ok(), referent);
                drop(file);
                no_staging_files(temp.path());
            }
        }
    }

    #[test]
    fn regular_file_recovery_does_not_remove_an_external_link() {
        for target in TARGETS {
            for contents in ["old", "new"] {
                let temp = tempfile::tempdir().unwrap();
                let _env = TestEnvGuard::isolated(temp.path());
                let path = temp.path().join("config");
                fs::write(&path, "old").unwrap();
                let mut file = NativeFile::observe(target, &path).unwrap();
                file.publish("new").unwrap();
                fs::write(temp.path().join("external"), contents).unwrap();
                fs::remove_file(&path).unwrap();
                symlink("external", &path).unwrap();
                assert!(file.rollback().is_err());
                assert_eq!(fs::read_link(&path).unwrap(), Path::new("external"));
                assert_eq!(fs::read_to_string(&path).unwrap(), contents);
            }
        }
    }

    #[test]
    fn successful_publication_cleans_staging_and_cycles_fail_before_writing() {
        for target in TARGETS {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            let path = temp.path().join("config");
            symlink("source", &path).unwrap();
            let mut file = NativeFile::observe(target, &path).unwrap();
            file.publish("new").unwrap();
            assert!(file.publish("again").is_err());
            drop(file);
            assert!(!path.is_symlink());
            assert_eq!(fs::read_to_string(&path).unwrap(), "new");
            no_staging_files(temp.path());
            fs::remove_file(&path).unwrap();
            symlink("config", &path).unwrap();
            assert!(NativeFile::observe(target, &path).is_err());
            assert_eq!(fs::read_link(&path).unwrap(), Path::new("config"));
        }
    }
}
