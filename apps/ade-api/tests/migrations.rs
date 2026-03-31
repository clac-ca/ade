use ade_api::embedded_migrations;

#[test]
fn embedded_migrations_use_small_ordered_versions() {
    let runner = embedded_migrations::migrations::runner();
    let mut migrations = runner
        .get_migrations()
        .iter()
        .map(|migration| (migration.version(), migration.name().to_string()))
        .collect::<Vec<_>>();
    migrations.sort_by_key(|(version, _)| *version);

    assert_eq!(migrations.len(), 3);
    assert_eq!(migrations[0].0, 1);
    assert_eq!(migrations[0].1, "initial_schema");
    assert_eq!(migrations[1].0, 2);
    assert_eq!(migrations[1].1, "runs");
    assert_eq!(migrations[2].0, 3);
    assert_eq!(migrations[2].1, "run_log_path");
}
