#!/usr/bin/env bash
python3 -c "
import json, sys
d = json.load(sys.stdin)
t = d['text']
print(json.dumps({
    'characters': len(t),
    'words': len(t.split()),
    'lines': t.count('\n') + (1 if t and not t.endswith('\n') else 0)
}))
"
