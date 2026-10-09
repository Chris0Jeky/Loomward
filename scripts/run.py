"""Run directly from a source checkout; no editable install is required."""
import sys
from pathlib import Path
if sys.version_info < (3,11):
    raise SystemExit('Loomward reference requires Python 3.11 or later')
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'python'))
from loomward.__main__ import main
if __name__=='__main__':raise SystemExit(main())
