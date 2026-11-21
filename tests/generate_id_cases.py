"""Generate exact-ID regressions with the independent sibling oracle.

Usage:
    python tests/generate_id_cases.py --output /tmp/id-cases
    python tests/generate_id_cases.py --output /tmp/id-stress --large

Default: five small codec cases. --large also adds four >4 MiB event files
with duplicate event and person names across files, for multi-worker remapping.
Generated inputs and expected CSVs are temporary test data, not repository files.
"""
import argparse
import csv
import datetime as dt
import json
import random
from pathlib import Path

from oracle import DAY, EPOCH, timestamp, write_expected

HEADERS = {
    'events': 'event_id,user_id,ts,ingest_ts,event_type,product_id,price_cents',
    'clicks': 'click_id,user_id,campaign_id,ts',
    'links': 'user_id_a,user_id_b',
}


def iso(ms):
    return (EPOCH + dt.timedelta(milliseconds=ms)).strftime('%Y-%m-%dT%H:%M:%SZ')


def encode(row, rng=None):
    fields = []
    for value in row:
        value = str(value)
        if rng is not None and rng.random() < .4:
            value = '"' + value + '"'
        fields.append(value)
    return ','.join(fields)


def build(base, name, events=(), clicks=(), links=(), start='2026-10-01T00:00:00Z', products=None, campaigns=None, seed=0):
    root = base / name / 'input'
    root.mkdir(parents=True, exist_ok=True)
    products = products or [('p1', 'shoes'), ('p2', 'books'), ('p3', 'é'), ('p4', 'z')]
    campaigns = campaigns or [('c_1', 'search'), ('c_2', 'social'), ('c_3', 'email')]
    for table, values in [('products', products), ('campaigns', campaigns)]:
        header = 'product_id,category' if table == 'products' else 'campaign_id,channel'
        (root / (table + '.csv')).write_bytes((header + '\n' + '\n'.join(encode(r) for r in values) + '\n').encode())
    manifest = {'window_start': start, 'window_end': iso(timestamp(start) + 14*DAY)}
    (root / 'manifest.json').write_text(json.dumps(manifest), encoding='utf-8')
    rng = random.Random(seed)
    for table, values in [('events', events), ('clicks', clicks), ('links', links)]:
        folder = root / table
        folder.mkdir(exist_ok=True)
        files = [[] for _ in range(5)]
        for row in values:
            line = row if isinstance(row, str) else encode(row, rng)
            files[rng.randrange(4)].append(line)
        for i, lines in enumerate(files):
            rng.shuffle(lines)
            (folder / f'arbitrary-name-{i}.csv').write_bytes((HEADERS[table] + '\n' + '\n'.join(lines) + ('\n' if lines else '')).encode('utf-8'))
        (folder / 'ignore.txt').write_text('not a CSV\n')
    write_expected(root, base / name / 'expected')



def person_cases(root):
    t="2026-10-02T00:00:00Z"
    names=["\x7f"*9,"\0"*9,"\0"*8,"A"*9,"A"*10,"Ω","0","00","000"," \r\t","uAA","UAA","prefix-000"]
    events=[[f"e{i}",name,t,t,"purchase","p1",str(i+1)] for i,name in enumerate(names)]
    clicks=[[f"c{i}",name,"c_1",t] for i,name in enumerate(names)]
    build(root,"I01_inline_and_fallback",events,clicks,seed=11)
    pairs=[("z"*10,"a"),("Ω","\0"*9),("A"*10,"z"),("\x7f"*9,"🦆"),("é"," a\t"),("ZZZZZZZZZZZ","y")]
    events=[];clicks=[]
    for i,(a,b) in enumerate(pairs):
     events.append([f"a{i}",b,t,t,"purchase","p1",str(i+1)])
     clicks.append([f"c{i}",a,"c_2",t])
    build(root,"I02_cross_encoding_aliases",events,clicks,pairs,seed=29)
    names=["unlinked-long-name","unlinked-Ω-name","\x7f"*9,"\0"*9,"u1","u01"]
    events=[];clicks=[]
    for i in range(64):
     name=names[i%len(names)]
     events.append([f"e{i}",name,t,t,"purchase","p2",str(i+1)])
     clicks.append([f"c{i}",name,"c_3",t])
    events.append(["dupe","\x7f"*9,"bad",t,"view","p1",""])
    events.append(["dupe","u1",t,"2026-10-02T00:00:01Z","view","p1",""])
    clicks.append(["orphan","onlyclick","c_1",t])
    build(root,"I03_repeated_names_and_invalid_dedup",events,clicks,seed=71)


def event_cases(root):
    t = timestamp('2026-10-03T10:00:00Z')
    def event(eid, user, ingest, price, valid=True):
        return [eid,user,str(t) if valid else 'invalid',iso(ingest),'purchase','p1',str(price)]

    names = ['\0'*length for length in range(1,10)] + ['\x7f'*9,'1','01','001','0001','a\r','a\0','a ']
    events=[]
    for index,name in enumerate(names):
        events += [event(name,'person-Ω',t,index+1),event(name,'wrong',t+1000,9999)]
    build(root,'E01_inline_event_lengths_and_all_bits',events=events,seed=184)

    names=['1234567890','123456789\0','123456789\r','123456789 ','Ω','ω','00000000001','0000000001']
    names += [f'long-shared-event-{i:03}' for i in range(80)]
    events=[]
    for index,name in enumerate(names):
        events += [event(name,'person-Ω',t+1000,index+1),event(name,'person-ω',t+2000,9000)]
        if index%3==0:
            events += [event(name,'invalid-user',t,99999,False)]
    build(root,'E02_fallback_event_exact_names_and_tombstones',events=events,seed=281)


def line(values, salt):
    # All commas/quotes/newlines remain CSV syntax, never part of a value.
    return ",".join('"' + value + '"' if (salt + j) % 3 == 0 else value
                    for j, value in enumerate(map(str, values)))


def large_case(case):
    data = case / "input"
    data.mkdir(parents=True, exist_ok=True)
    begin = timestamp("2026-10-01T00:00:00Z")
    (data / "manifest.json").write_text(json.dumps({"window_start": iso(begin), "window_end": iso(begin + 14 * DAY)}))
    (data / "products.csv").write_bytes("product_id,category\np1,é\np2,z\np3,books\n".encode())
    (data / "campaigns.csv").write_bytes(b"campaign_id,channel\nc_1,search\nc_2,social\nc_3,email\n")

    users = ([f"unlinked-person-{i}-shared-across-files" for i in range(32)]
             + [f"unlinked-Ω-person-{i}-shared-across-files" for i in range(32)]
             + ["\0" * n for n in range(1, 10)]
             + ["\x7f" * 9, "A" * 9, "A" * 10, "0", "00", "000", "Ω", " \r\t"])
    links = []
    unused_minima = []
    for i in range(16):
        a = f"a{i}"
        b = f"linked-Ω-alias-{i}-long-name"
        minimum = "\x01" + (str(i) if i % 2 == 0 else f"unused-long-Ω-minimum-{i}")
        unused_minima.append(minimum)
        users.extend([a, b])
        links.extend([[a, b], [b, minimum], [b, a], [a, a]])
    tables = {name: [[] for _ in range(4)] for name in HEADERS}
    for i, row in enumerate(links):
        tables["links"][i % 4].append(line(row, i))
    tables["links"][0].extend(["", "bad,", '"bad"quote,a0', "wrong,count,here"])

    special = ["\0" * n for n in range(1, 10)] + ["\x7f" * 9, "B" * 9, "B" * 10, "Ω", "0", "00", "000"]
    invalid_first = 0
    for i in range(65_536):
        eid = special[i] if i < len(special) else f"event-Ω-{i}-" + "abcdefghijklmnop" * 4
        user = users[i % len(users)]
        cycle = i // len(users)
        day = cycle % 14
        slot = (cycle // 14) % 6
        at = begin + day * DAY + slot * 3 * 3_600_000 + (i % 29) * 500
        kind = "purchase" if i % 7 == 0 else ("cart" if i % 5 == 0 else "view")
        price = "000" + str(i % 997 + 1) if kind == "purchase" else "ignored-price-padding-" * 3
        row = [eid, user, str(at), iso(at), kind, f"p{i % 3 + 1}", price]
        later = row.copy()
        later[3] = iso(at + 1000)
        # Invalid earliest values must remain tombstones before global dedup.
        if i >= len(special) and i % 29 == 0:
            row[2] = "invalid-earliest"
            invalid_first += 1
        if i >= len(special) and i % 17 == 0:
            later[1] = users[(i + 7) % len(users)]
        first_file = i % 4
        tables["events"][first_file].append(line(row, i))
        tables["events"][(first_file + 1) % 4].append(line(later, i + 1))

    # Repeated long click IDs with exact duplicates, timestamp ties, seven-day
    # boundary clicks, and linked aliases used from their other side.
    for person, user in enumerate(users):
        for day in range(14):
            at = begin + day * DAY
            for slot, delta in enumerate([-7 * DAY, -1, 0, 3 * 3_600_000, 3 * 3_600_000]):
                cid = f"click-Ω-{person}-{day}-{slot}-shared-across-files"
                campaign = ["c_1", "c_2", "c_3", "c_2", "c_1"][slot]
                row = [cid, user, campaign, str(at + delta)]
                part = (person + day + slot) % 4
                tables["clicks"][part].append(line(row, person + slot))
                tables["clicks"][(part + 2) % 4].append(line(row, person + slot + 1))
    tables["clicks"][3].append(line(["orphan", "onlyclick", "c_1", str(begin)], 0))

    sizes = {}
    for table, files in tables.items():
        folder = data / table
        folder.mkdir(exist_ok=True)
        for part, values in enumerate(files):
            random.Random(90_000 + part + len(table)).shuffle(values)
            path = folder / f"part-{part:05}.csv"
            path.write_bytes((HEADERS[table] + "\n" + "\n".join(values) + "\n").encode("utf-8"))
            sizes[f"{table}/{path.name}"] = path.stat().st_size
        (folder / "header-only.csv").write_bytes((HEADERS[table] + "\n").encode())
    event_sizes = [sizes[f"events/part-{i:05}.csv"] for i in range(4)]
    assert min(event_sizes) > 4 * 1024 * 1024, event_sizes
    assert not any(name in users for name in unused_minima)
    write_expected(data, case / "expected")
    report = {"unique_event_ids": 65_536, "event_rows": sum(map(len, tables["events"])),
              "invalid_earliest_ids": invalid_first, "persons_in_event_rotation": len(users),
              "unused_linked_minimum_names": len(unused_minima), "event_file_bytes": event_sizes,
              "total_input_bytes": sum(p.stat().st_size for p in data.rglob("*") if p.is_file()),
              "oracle": "oracle.py"}
    (case / "generation.json").write_text(json.dumps(report, indent=2))
    return report


EXPECTED = {
    "I01_inline_and_fallback": (13, 13, 1),
    "I02_cross_encoding_aliases": (6, 6, 1),
    "I03_repeated_names_and_invalid_dedup": (66, 6, 1),
    "E01_inline_event_lengths_and_all_bits": (34, 1, 1),
    "E02_fallback_event_exact_names_and_tombstones": (206, 1, 1),
    "L01_cross_morsel_fallback": (131_072, 1_358, 42),
}


def fixture_counts(case):
    data = case / "input"
    counts = {}
    for table in HEADERS:
        counts[table + "_input_rows"] = sum(
            sum(bool(line) for line in file.read_bytes().split(b"\n")[1:])
            for file in (data / table).glob("*.csv")
        )
    for table in ["user_daily", "campaign_daily"]:
        with (case / "expected" / (table + ".csv")).open(encoding="utf-8", newline="") as handle:
            counts[table + "_rows"] = sum(1 for _ in csv.reader(handle)) - 1
    actual = (counts["events_input_rows"], counts["user_daily_rows"], counts["campaign_daily_rows"])
    if actual != EXPECTED[case.name]:
        raise AssertionError(f"{case.name}: expected counts {EXPECTED[case.name]}, got {actual}")
    return counts


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--output", required=True, type=Path, help="new or empty fixture-root directory")
    parser.add_argument("--large", action="store_true", help="also generate the 131,072-row multi-morsel fallback case")
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() and (not output.is_dir() or any(output.iterdir())):
        parser.error("--output must be a new or empty directory")
    output.mkdir(parents=True, exist_ok=True)
    person_cases(output)
    event_cases(output)
    large = large_case(output / "L01_cross_morsel_fallback") if args.large else None
    report = {"oracle": "oracle.py", "case_count": 6 if args.large else 5,
              "cases": {case.name: fixture_counts(case) for case in sorted(output.iterdir()) if case.is_dir()}}
    if large is not None:
        report["large_case"] = large
    (output / "generation.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
