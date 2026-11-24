import json
import pytest
from analyze import summarize


def test_gap_and_integrity(tmp_path):
    file = tmp_path/"samples.jsonl"
    rows = [{"sequence":i,"sent_us":1_000_000+i*33_000,"received_us":1_010_000+i*33_000+(200_000 if i==2 else 0),
             "elapsed_ms":50+i*33,"bytes":1000} for i in range(3)]
    file.write_text("\n".join(json.dumps(row) for row in rows))
    stats = summarize(file,4,33)
    assert stats["delivery_ratio"] == .75 and stats["interruption_gaps"] == 1
    rows.append(rows[0])
    file.write_text("\n".join(json.dumps(row) for row in rows))
    with pytest.raises(ValueError,match="Duplicate"):
        summarize(file,4,33)
