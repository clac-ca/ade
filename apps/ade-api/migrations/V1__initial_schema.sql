IF NOT EXISTS (
  SELECT 1
  FROM sys.schemas
  WHERE name = N'ade'
)
BEGIN
  EXEC(N'CREATE SCHEMA [ade]');
END
