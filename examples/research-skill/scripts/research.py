#!/usr/bin/env python3
"""research-skill entrypoint (illustrative).

This example does not actually reach the network — it shows the *shape* of a
skill whose manifest declares `net` and `fs.write` capabilities. A real host
would grant those capabilities per its policy before invoking this.
"""

import os
import sys


def main(argv):
    url = argv[1] if len(argv) > 1 else "https://example.com"
    max_words = int(argv[2]) if len(argv) > 2 else 200

    # A capability-honoring skill would confine writes to its declared scope.
    out_dir = os.path.join(os.getcwd(), "reports")
    os.makedirs(out_dir, exist_ok=True)
    report_path = os.path.join(out_dir, "report.md")
    with open(report_path, "w", encoding="utf-8") as fh:
        fh.write(f"# Report for {url}\n\n(placeholder, capped at {max_words} words)\n")

    print(f"research-skill: wrote {report_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
