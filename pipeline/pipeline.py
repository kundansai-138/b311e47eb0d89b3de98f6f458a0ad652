"""Optional convenience launcher. The native executable writes Parquet directly."""
import os
from pathlib import Path

if __name__ == "__main__":
    root = Path(__file__).resolve().parents[1]
    executable = os.environ.get("SHOPPING_ENGINE", str(root / "target" / "release" / "shopping-sessions"))
    os.execv(executable, [executable])
