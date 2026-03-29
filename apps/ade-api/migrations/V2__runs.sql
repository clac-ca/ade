IF OBJECT_ID(N'ade.runs', N'U') IS NULL
BEGIN
  CREATE TABLE ade.runs (
    run_id NVARCHAR(128) NOT NULL PRIMARY KEY,
    workspace_id NVARCHAR(255) NOT NULL,
    config_version_id NVARCHAR(255) NOT NULL,
    input_path NVARCHAR(1024) NOT NULL,
    status NVARCHAR(32) NOT NULL,
    phase NVARCHAR(64) NULL,
    attempt_count INT NOT NULL CONSTRAINT DF_ade_runs_attempt_count DEFAULT (0),
    last_session_guid NVARCHAR(128) NULL,
    output_path NVARCHAR(1024) NULL,
    validation_issues_json NVARCHAR(MAX) NOT NULL,
    error_message NVARCHAR(MAX) NULL,
    created_at DATETIME2 NOT NULL CONSTRAINT DF_ade_runs_created_at DEFAULT (SYSUTCDATETIME()),
    updated_at DATETIME2 NOT NULL CONSTRAINT DF_ade_runs_updated_at DEFAULT (SYSUTCDATETIME())
  );

  CREATE INDEX IX_ade_runs_scope
    ON ade.runs (workspace_id, config_version_id, created_at DESC);
END

IF OBJECT_ID(N'ade.run_events', N'U') IS NULL
BEGIN
  CREATE TABLE ade.run_events (
    seq BIGINT IDENTITY(1,1) NOT NULL PRIMARY KEY,
    run_id NVARCHAR(128) NOT NULL,
    event_type NVARCHAR(32) NOT NULL,
    payload_json NVARCHAR(MAX) NOT NULL,
    created_at DATETIME2 NOT NULL CONSTRAINT DF_ade_run_events_created_at DEFAULT (SYSUTCDATETIME()),
    CONSTRAINT FK_ade_run_events_run_id
      FOREIGN KEY (run_id) REFERENCES ade.runs(run_id) ON DELETE CASCADE
  );

  CREATE INDEX IX_ade_run_events_run_id_seq
    ON ade.run_events (run_id, seq ASC);
END
