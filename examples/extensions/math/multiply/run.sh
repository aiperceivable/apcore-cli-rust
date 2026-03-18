#!/usr/bin/env bash
python3 -c "
import json, sys
d = json.load(sys.stdin)
print(json.dumps({'product': int(d['a']) * int(d['b'])}))
"
