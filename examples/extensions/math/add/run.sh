#!/usr/bin/env bash
python3 -c "
import json, sys
d = json.load(sys.stdin)
print(json.dumps({'sum': int(d['a']) + int(d['b'])}))
"
