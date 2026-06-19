#!/usr/bin/env python3
"""hello-skill entrypoint: print a greeting."""

import sys


def main(argv):
    name = argv[1] if len(argv) > 1 else "world"
    print(f"Hello, {name}! This greeting came from hello-skill.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
