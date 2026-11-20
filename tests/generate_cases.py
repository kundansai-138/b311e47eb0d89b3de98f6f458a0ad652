"""Build small adversarial fixtures, expected results from the independent oracle."""
import argparse
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


def explicit_cases(base):
    t = timestamp('2026-10-03T10:00:00Z')
    def event(eid, user='u1', at=t, kind='view', product='p1', price='', ingest=None):
        return [eid, user, str(at), iso(at if ingest is None else ingest), kind, product, price]
    # Arbitrarily many leading zeros are valid, even beyond Python's default integer conversion limit.
    build(base, 'X01_long_zeros', events=[
        ['e0', 'u1', '0'*6000+str(t), iso(t), 'purchase', 'p1', '0'*6000+'10000000'],
        ['e1', 'u1', '0'*6000+'86400000', iso(t), 'view', 'p1', ''],
        ['e2', 'u1', '9'*6000, iso(t), 'view', 'p1', ''],
        ['e3', 'u1', str(t), iso(t), 'purchase', 'p1', '0'*6000],
        event('e4', at=t+2_000_000, kind='purchase', price='00000001'),
    ], clicks=[['c0', 'u1', 'c_1', '0'*6000+str(t)]])
    # Hundreds of links, both orientations, reused shared node, unused bytewise-minimal ID.
    links = [(f'user{i+1}', f'user{i}') for i in range(700)]
    links += [('user0', '\x00minimum'), ('user400', 'shared'), ('outsider', 'shared'), ('user_id_a', 'user_id_b'), ('user_id_b', '\tlinked')]
    links += [('user20', 'user20'), ('user1', 'user0'), ('', 'must-not-join'), '"bad"quote,user0', 'bad,user0,extra']
    build(base, 'X02_long_links_bytes', links=links, events=[event('e0', 'user700'), event('e1', 'outsider', at=t+1000, kind='purchase', product='p2', price=99), event('e2', 'user_id_a'), event('e3', 'u\r1'), event('e4', 'u\x001'), event('e5', 'u\t1'), event('e6', 'é')], clicks=[['k0', 'user0', 'c_2', str(t)]])
    # A single purchase's standalone revenue need not identify the day's top category.
    build(base, 'X03_daily_category', events=[
        event('e0', kind='purchase', product='p1', price=400),
        event('e1', at=t+2_000_000, kind='purchase', product='p2', price=600),
        event('e2', at=t+4_000_000, kind='purchase', product='p1', price=200),
        event('e3', 'u2', kind='purchase', product='p3', price=100),
        event('e4', 'u2', at=t+2_000_000, kind='purchase', product='p4', price=100),
    ])
    # Last field CR must remain; malformed quote suffix prevents a row poisoning dedup.
    build(base, 'X04_quotes_cr_dedup', events=[
        'x,u1,2026-10-03T10:00:00Z,2026-10-03T10:00:00Z,view,p1,"ignored"\r',
        ['x', 'u1', str(t), iso(t+1000), 'purchase', 'p1', '30'],
        'a,u1,2026-10-03T10:00:00Z,2026-10-03T10:00:00Z,view,p1,ignored\r',
        'b,u1,2026-10-03T10:00:00Z,2026-10-03T10:00:00Z,purchase,p1,100\r',
        ['same', 'bad', 'invalid', iso(t), 'view', 'p1', ''],
        ['same', 'u1', str(t), iso(t+1000), 'purchase', 'p1', '9999'],
        ['different', 'u1', str(t), iso(t), 'purchase', 'p1', '10'],
        ['different', 'u2', str(t), iso(t+1000), 'purchase', 'p2', '1000'],
        'empty,u3,2026-10-03T10:00:00Z,2026-10-03T10:00:00Z,view,"",0',
    ], clicks=['k,u1,c_1,2026-10-03T09:00:00Z\r'])
    # New and expiring assist intervals, repeated clicks, inclusive edges and lexical tie.
    begin = timestamp('2026-10-01T00:00:00Z')
    times = [begin, begin+1_800_001, begin+DAY, begin+7*DAY, begin+7*DAY+1_800_001, begin+8*DAY]
    build(base, 'X05_assist_expiry', events=[event(f'e{i}', at=v) for i,v in enumerate(times)], clicks=[
        ['before', 'u1', 'c_3', str(begin-7*DAY-1)],
        ['edge', 'u1', 'c_1', str(begin-7*DAY)],
        ['k2', 'u1', 'c_2', str(begin)], ['k10', 'u1', 'c_3', str(begin)],
        ['k3', 'u1', 'c_2', str(begin+1)], ['k3', 'u1', 'c_2', str(begin+1)],
        ['later', 'u1', 'c_1', str(begin+7*DAY+1_800_001)],
    ])
    # Leap-year validation and offsets that move dates in both directions.
    start = '2028-02-23T00:00:00Z'
    forms = ['2028-02-29T23:59:59Z', '2028-02-30T00:00:00Z', '2027-02-29T00:00:00Z',
             '2028-03-01T00:30:00+01:00', '2028-02-29T23:30:00-01:00',
             '2028-02-29T00:00:00+23:59', '2028-02-29T00:00:00-23:59',
             '2028-02-29T00:00:00-00:00', '2028-02-29T00:00:00+24:00',
             '2028-02-29T00:00:00+00:60', '2028-02-29T00:00:00+0000',
             '2028-02-29T24:00:00Z', '2028-02-29T00:00:60Z', '2028-02-00T00:00:00Z',
             '２０２８-02-29T00:00:00Z', '2028-02-29T00:00:00Z\r']
    build(base, 'X06_calendar_offsets', start=start, events=[[f'e{i}', f'u{i}', form, '2028-02-23T00:00:00Z', 'view', 'p1', ''] for i,form in enumerate(forms)])
    build(base, 'X07_empty', events=[], clicks=[], links=[])
    build(base, 'X08_all_invalid', events=[event('bad', kind='Purchase'),event('bad-price',kind='purchase',price='1.0'),event('',kind='view'),['out','u1',str(begin-1),iso(begin),'view','p1','']],clicks=[['x','u1','invalid',str(begin)]])
    # Repeated equal timestamps around a strict session boundary and cross-midnight grouping.
    midnight = timestamp('2026-10-04T00:00:00Z')
    build(base, 'X09_equal_times_midnight', events=[
        event('a', at=midnight-1_200_000),
        event('b', 'u2', at=midnight+600_000,kind='purchase',product='p1',price=500),
        event('c', at=midnight+600_000,kind='purchase',product='p2',price=500),
        event('d', at=midnight+2_400_001), event('e','u2',at=midnight+2_400_001),
        event('f', at=midnight+4_200_001),
    ],links=[('u1','u2')],clicks=[['k0','u2','c_1',str(midnight+2_400_001)],['k1','u1','c_2',str(midnight)]])
    # Ingest exactly 24 h, 24 h + 1 ms (event carries ms), and before the event.
    build(base, 'X10_ingest_boundaries', events=[
        event('a', at=t, ingest=t+DAY), event('b',at=t+999,ingest=t+DAY+1000),
        event('c',at=t+1001,ingest=t+DAY+1000),event('d',at=t,ingest=t-DAY*2),
    ])
    # A fourteen-day window starting late in the day touches fifteen UTC dates.
    # The final millisecond belongs to the last date; window_end itself is excluded.
    partial_start = '2026-10-01T23:30:00Z'
    partial_begin = timestamp(partial_start)
    partial_times = [partial_begin, partial_begin+3_600_000,
                     partial_begin+14*DAY-1, partial_begin+14*DAY]
    build(base, 'X11_partial_day_window', start=partial_start,
          events=[event(f'e{i}', at=at, kind='purchase', price='1')
                  for i, at in enumerate(partial_times)],
          clicks=[['k0', 'u1', 'c_2', str(partial_begin)]])
    # Many catalog campaigns and few observed rows catch sparse aggregation paths
    # and require preserving a campaign that assisted but received no credit.
    sparse_begin = timestamp('2026-10-01T00:00:00Z')
    build(base, 'X12_sparse_campaign_catalog',
          campaigns=[(f'c_{i}', 'channel') for i in range(21_000)],
          events=[event('e0', at=sparse_begin, kind='purchase', price='25'),
                  event('e1', at=sparse_begin+DAY)],
          clicks=[['k0', 'u1', 'c_0', str(sparse_begin-1)],
                  ['k1', 'u1', 'c_20999', str(sparse_begin)]])


def random_cases(base, count):
    for seed in range(count):
        rng=random.Random(seed)
        start = '2028-02-23T00:00:00Z' if seed % 3 == 0 else '2026-10-01T00:00:00Z'
        begin=timestamp(start)
        users=['u1','u2','u3','U1',' u1','u1 ','\tu1','é','Ω','u\r1','u\x001']
        links=[]
        for _ in range(12):
            links.append((rng.choice(users+['','never-used']),rng.choice(users+['','unused'])))
        links.extend(['bad"quote,u1','u1,u2,extra','', 'user_id_a,user_id_b'])
        events=[]
        for i in range(250):
            at=begin+rng.choice([0,1,DAY-1,DAY,7*DAY,14*DAY-1,14*DAY, -1,rng.randrange(14*DAY)])
            form=rng.randrange(5)
            ts = str(at) if form<2 else iso(at)
            if form==3:
                offset=rng.choice([-1439,-720,-330,0,330,720,1439])
                absolute=abs(offset)
                ts=iso(at+offset*60_000)[:-1]+('+' if offset>=0 else '-')+f'{absolute//60:02}:{absolute%60:02}'
            if form==4:
                ts=rng.choice(['000'+str(at),'1e12','',str(at)+'.0','2026-02-30T10:00:00Z','٢٥٠','-1'])
            ingest=iso(at+rng.choice([0,1000,-DAY,DAY,DAY+1000]))
            row=[f'e{i}',rng.choice(users+['']),ts,ingest,rng.choice(['view']*5+['cart','purchase']*3+['Purchase','view ']),rng.choice(['p1','p2','p3','p4','unknown','',' p1']),rng.choice(['','0','1','100','000100','10000000','10000001','-100','١٠٠','100\r'])]
            events.append(row)
            if rng.random()<.25:
                duplicate=row.copy()
                duplicate[3]=iso(timestamp(ingest)+rng.choice([-1000,1000,30_000]))
                duplicate[1]=rng.choice(users)
                if rng.random()<.3:duplicate[2]='invalid-earlier-or-later'
                events.append(duplicate)
        events.extend(['','malformed,fields','"unclosed,u1,123,2026-10-01T00:00:00Z,view,p1,','x,u1,123,2026-10-01T00:00:00Z,view,"p1"extra,'])
        clicks=[]
        for i in range(120):
            at=begin+rng.choice([-7*DAY,-7*DAY-1,-1,0,1,DAY,7*DAY,14*DAY-1,14*DAY,rng.randrange(-7*DAY,14*DAY)])
            row=[f'k{i}',rng.choice(users+['']),rng.choice(['c_1','c_2','c_3','unknown',' c_1']),rng.choice([str(at),'00'+str(at),iso(at),'bad'])]
            clicks.append(row)
            if rng.random()<.15:clicks.append(row.copy())
        clicks+=['bad,u1,c_1,2026-10-01T00:00:00Z\r','bad"quote,u1,c_1,2026-10-01T00:00:00Z']
        build(base,f'R{seed:03}',events,clicks,links,start,seed=seed)


if __name__=='__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('--output',type=Path,default=Path(__file__).parent/'cases')
    parser.add_argument('--random-count',type=int,default=32)
    args=parser.parse_args()
    explicit_cases(args.output)
    random_cases(args.output,args.random_count)
    print(f'Wrote 12 explicit and {args.random_count} randomized cases to {args.output}')
