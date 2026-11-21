"""Generate one small exact-catalog regression using the independent oracle.

Copy beside tests/generate_cases.py and tests/oracle.py, then run:
    python tests/generate_catalog_cases.py --output /tmp/catalog-cases

Generated input CSVs and oracle expected CSVs are temporary test data, not
repository files. All strings follow the task's LF-only, whole-field CSV rules.
"""
import argparse
from pathlib import Path

from generate_cases import build, iso
from oracle import timestamp


def catalog_case(output):
    # Different lengths must remain distinct even when every byte is zero.
    # The 9/10-byte boundary exercises both inline and full-string lookup.
    names = ["0" * length for length in range(1, 11)]
    names += ["\0" * length for length in range(1, 11)]
    names += [
        "\x7f" * 9,
        "\x7f" * 10,
        "A",
        "a",
        "a\r",
        "a\t",
        " a",
        "a ",
        "\u03a9",
        "long-\u03a9-product",
        "123456789",
        "1234567890",
    ]
    at = timestamp("2026-10-03T10:00:00Z")
    products = [(name, "category-" + str(index % 7))
                for index, name in enumerate(names)]
    campaigns = [(name, "channel-" + str(index % 5))
                 for index, name in enumerate(names)]
    events = [["event" + str(index), "person" + str(index), str(at), iso(at),
               "purchase", name, str(index + 1)]
              for index, name in enumerate(names)]
    clicks = [["click" + str(index), "person" + str(index), name, str(at)]
              for index, name in enumerate(names)]
    # Unknown product/campaign lookups must stay absent in either map domain.
    events.append(["unknown", "person-unknown", str(at), iso(at),
                   "purchase", "unknown-product", "11"])
    clicks.append(["unknown", "person-unknown", "unknown-campaign", str(at)])
    name = "C01_exact_catalog_domains"
    # build writes the fixture, then calls oracle.write_expected on its inputs.
    build(output, name, events=events, clicks=clicks,
          products=products, campaigns=campaigns)
    return name


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True,
                        help="Directory receiving the generated fixture")
    args = parser.parse_args()
    name = catalog_case(args.output)
    print(f"Generated 1 catalog case: {args.output / name}")


if __name__ == "__main__":
    main()
