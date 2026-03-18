#!/usr/bin/env bash
python3 -c "
import json, sys, os
d = json.load(sys.stdin)
name = d['name']
default = d.get('default')
value = os.environ.get(name)
exists = value is not None
if not exists and default is not None:
    value = default
print(json.dumps({
    'name': name,
    'value': value if value is not None else '',
    'exists': exists
}))
"
