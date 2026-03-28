def score_email_header(column) -> float:
    header = (column.header or "").strip().lower()
    return 1.0 if header == "email" else 0.0


def register(config) -> None:
    config.detector("email", score_email_header)
