#!/usr/bin/env bash
python3 -c "
import json, sys
d = json.load(sys.stdin)
print(json.dumps({'result': d['text'][::-1]}))
"
