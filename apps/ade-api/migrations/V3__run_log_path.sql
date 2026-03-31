IF COL_LENGTH(N'ade.runs', N'log_path') IS NULL
BEGIN
  ALTER TABLE ade.runs
    ADD log_path NVARCHAR(1024) NULL;
END
