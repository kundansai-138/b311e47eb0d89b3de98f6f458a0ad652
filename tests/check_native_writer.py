"""Independent PyArrow readback of native writer row-group/null boundaries."""
from pathlib import Path
import sys
import pyarrow as pa
import pyarrow.parquet as pq

root = Path(sys.argv[1])
file = pq.ParquetFile(root / "boundary.parquet")
assert file.num_row_groups == 2
table = file.read()
assert table.num_rows == 131_077
assert table.schema.field("day").type == pa.date32()
assert table["top_category"].null_count == 131_074
for i, row in enumerate(table.to_pylist()):
    assert row["user_id"] == f"u{i}\0\r\tΩ"
    assert row["day"].toordinal() == 719163 + i % 14
    assert row["sessions"] == i + 1
    assert row["events"] == i * 2
    assert row["revenue_cents"] == i * 100
    assert row["top_category"] == ("é\r" if i >= 131_072 and i % 2 == 0 else None)
for name in ["empty_user", "empty_campaign"]:
    empty = pq.read_table(root / f"{name}.parquet")
    assert empty.num_rows == 0
    assert empty.schema.field("day").type == pa.date32()
print("Exact native Parquet readback passed: two row groups, all-null group, mixed nulls, empty tables, Unicode/CR/NUL strings")
