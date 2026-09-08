use super::*;

#[test]
#[ignore = "manual bounded timing comparison; not a CI performance threshold"]
fn measure_import_scaling() {
    // The former standalone Gemini service boundary, kept only as a timing
    // reference. It is unsafe for shared-consumer persistence and is not an
    // alternative production path.
    fn legacy_import(state: &AppState) -> Result<usize, AppError> {
        let mut config = state.config.write()?;
        let count = crate::mcp::import_from_gemini(&mut config)?;
        drop(config);
        state.save()?;
        Ok(count)
    }
    type Import = fn(&AppState) -> Result<usize, AppError>;
    for size in [10, 100, 500] {
        for (name, import) in [
            ("guarded", McpService::import_from_gemini as Import),
            ("legacy", legacy_import as Import),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let _env = TestEnvGuard::isolated(temp.path());
            let path = crate::gemini_config::get_gemini_settings_path();
            assert!(path.starts_with(temp.path()));
            let entries = (0..size)
                .map(|index| {
                    (
                        format!("fixture-{index:04}"),
                        json!({"command":"not-executed-scale"}),
                    )
                })
                .collect::<serde_json::Map<_, _>>();
            let native = json!({"mcpServers":entries,"fixture":"keep"}).to_string();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &native).unwrap();
            let state = AppState::new(Arc::new(Database::init().unwrap()));
            for (phase, count) in [("new", size), ("repeated", 0)] {
                let start = Instant::now();
                let actual = import(&state).unwrap();
                let elapsed = start.elapsed();
                assert_eq!(actual, count);
                assert_eq!(state.db.get_all_mcp_servers().unwrap().len(), size);
                assert_eq!(fs::read(&path).unwrap(), native.as_bytes());
                eprintln!(
                    "mcp_import_scale size={size} method={name} phase={phase} micros={}",
                    elapsed.as_micros()
                );
            }
        }
    }
}
