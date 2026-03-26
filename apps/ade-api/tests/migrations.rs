use ade_api::embedded_migrations;

#[test]
fn embedded_migrations_support_timestamp_versions() {
    let runner = embedded_migrations::migrations::runner();
    let migrations = runner.get_migrations();

    assert_eq!(migrations.len(), 1);
    assert_eq!(migrations[0].version() as i64, 20_260_324_120_000);
    assert_eq!(migrations[0].name(), "initial_schema");
}
