#!/usr/bin/env python3
"""Fail closed while pytest touched-test stability receipts are unsupported."""
import sys

if __name__ == '__main__':
    print('verified-pytest: touched-test stability receipts are not supported yet', file=sys.stderr)
    sys.exit(2)
