from ade_engine import load_config


def test_ade_config_package_loads_successfully() -> None:
    config = load_config("ade_config", name="ade-config")

    assert config.name == "ade-config"
    assert sorted(config.fields) == ["email", "full_name"]
    assert [fn.__name__ for fn in config.fields["email"].detectors] == [
        "score_email_header",
        "score_email_values",
    ]
    assert [fn.__name__ for fn in config.hooks["on_table_written"]] == [
        "log_table_written",
        "append_summary",
    ]
