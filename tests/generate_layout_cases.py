"""Independent-oracle cases for packed domains and bounded writer groups."""
import argparse
import json
from pathlib import Path

from generate_cases import build, iso
from oracle import DAY, timestamp, write_expected


def generate(base):
    start = timestamp('2026-10-01T00:00:00Z')

    def event(name, at, price=10_000_000, product='missing'):
        return [name, 'u', str(at), iso(at), 'purchase', product, str(price)]

    def case(name, begin, end, events, products=None, clicks=()):
        build(base, name, events=events, clicks=clicks, products=products, start=iso(begin))
        data = base / name / 'input'
        # Numeric epoch preserves millisecond dispatch boundaries exactly.
        value = lambda ts: str(ts) if ts >= 0 else iso(ts)
        (data / 'manifest.json').write_text(json.dumps({
            'window_start': value(begin), 'window_end': value(end)}), encoding='utf-8')
        write_expected(data, base / name / 'expected')

    for count in [255, 256]:
        products = [(f'p{i}', f'c{i:03}') for i in range(count)]
        events = [event('unknown', start+1000), event('first', start+2000, product='p0'),
                  event('last', start+1_802_001, 1, f'p{count-1}')]
        case(f'P_catalog_{count+1}', start, start+14*DAY, events, products=products)
    for span in [2**32-1, 2**32]:
        case(f'P_event_span_{span}', start, start+span,
             [event('first', start), event('last', start+span-1), event('after', start+span)])
    negative = timestamp('1969-12-20T12:34:56Z')
    events = [['negative', 'u', iso(negative+1000), iso(negative+1000), 'purchase', 'missing', '10000000'],
              ['boundary', 'u', '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z', 'purchase', 'p1', '1'],
              ['invalid', 'u', 'invalid', iso(negative), 'purchase', 'p1', '999'],
              ['invalid', 'u', iso(negative+1000), iso(negative+1000), 'purchase', 'p1', '999']]
    case('P_negative_epoch', negative, negative+14*DAY, events,
         clicks=[['click', 'u', 'c_1', iso(negative-7*DAY)]])
    for span in [2**32-1, 2**32]:
        end = start + span - 7*DAY
        clicks = [['inclusive', 'u', 'c_1', str(start-7*DAY)],
                  ['expired', 'u', 'c_2', str(start-7*DAY-1)],
                  ['final', 'u', 'c_2', str(end-1)], ['excluded', 'u', 'c_1', str(end)]]
        case(f'P_click_span_{span}', start, end,
             [event('first', start, 1, 'p1'), event('last', end-1, 2, 'p1')], clicks=clicks)


def writer_case(base):
    data = base / 'P_writer_row_groups' / 'input'
    data.mkdir(parents=True)
    (data / 'manifest.json').write_text(json.dumps({
        'window_start': '2026-10-01T00:00:00Z', 'window_end': '2026-10-15T00:00:00Z'}))
    (data / 'products.csv').write_bytes('product_id,category\np1,é\n'.encode())
    (data / 'campaigns.csv').write_bytes(b'campaign_id,channel\nc_1,search\n')
    for name, header in [('clicks', 'click_id,user_id,campaign_id,ts'), ('links', 'user_id_a,user_id_b')]:
        (data / name).mkdir()
        (data / name / 'empty.csv').write_bytes((header+'\n').encode())
    (data / 'events').mkdir()
    streams = [(data / 'events' / f'part-{part}.csv').open('wb') for part in range(2)]
    try:
        for stream in streams:
            stream.write(b'event_id,user_id,ts,ingest_ts,event_type,product_id,price_cents\n')
        for index in range(131_077):
            kind = 'purchase' if index % 3 == 0 else 'view'
            price = str(index % 100 + 1) if kind == 'purchase' else ''
            row = f'e{index:08},u{index:08},2026-10-02T00:00:00Z,2026-10-02T00:00:00Z,{kind},p1,{price}\n'
            streams[index % 2].write(row.encode())
    finally:
        for stream in streams:
            stream.close()
    write_expected(data, base / 'P_writer_row_groups' / 'expected')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--writer', action='store_true')
    args = parser.parse_args()
    generate(args.output)
    if args.writer:
        writer_case(args.output)
    print(f'Generated {7 + int(args.writer)} independent layout/writer cases')
