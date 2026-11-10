"""The report must not turn incomplete or non-equivalent samples into ratios."""

import csv
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


HEADER = "mode,case,input_bytes,iterations,sample,engine,ns_per_op,equivalent\n"


class ReportTests(unittest.TestCase):
    def run_summary(self, *rows):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "raw.csv"
            path.write_text(HEADER + "\n".join(rows) + "\n", encoding="utf-8")
            return subprocess.run(
                [sys.executable, str(Path(__file__).with_name("summarize.py")), str(path)],
                capture_output=True, text=True, check=False,
            )

    def test_valid_pair_uses_reference_over_zmime(self):
        result = self.run_summary(
            "headers,tiny,16,100,0,zmime,20,true",
            "headers,tiny,16,100,0,mail-parser-1.0.1,60,true",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(next(csv.DictReader(io.StringIO(result.stdout)))["ratio_reference_over_zmime"], "3.000")

    def test_different_work_never_has_ratio(self):
        result = self.run_summary(
            "fresh,tiny,16,100,0,zmime,20,false",
            "fresh,tiny,16,100,0,mail-parser-1.0.1,60,false",
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(next(csv.DictReader(io.StringIO(result.stdout)))["ratio_reference_over_zmime"], "")

    def test_incomplete_pairs_and_inconsistent_metadata_fail(self):
        first = "headers,tiny,16,100,0,zmime,20,true"
        for second in (
            "headers,tiny,16,100,1,mail-parser-1.0.1,60,true",
            "headers,tiny,16,200,0,mail-parser-1.0.1,60,true",
            "headers,tiny,16,100,0,mail-parser-1.0.1,60,false",
            "headers,tiny,32,100,0,mail-parser-1.0.1,60,true",
            "headers,tiny,16,100,0,mail-parser-1.0.1,nan,true",
            "headers,tiny,16,100,0,mail-parser-1.0.1,0,true",
            first,
            "",
        ):
            with self.subTest(second=second):
                self.assertNotEqual(self.run_summary(first, second).returncode, 0)


if __name__ == "__main__":
    unittest.main()
