def score_data(row) -> float:
    non_empty = [value for value in row.values if value not in (None, "")]
    return 0.7 if len(non_empty) >= 2 else 0.0


def register(config) -> None:
    config.row_detector("data", score_data)
