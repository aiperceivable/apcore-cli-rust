#!/usr/bin/env bash
python3 -c "
import json, sys, os
d = json.load(sys.stdin)
p = d.get('path') or '/'
st = os.statvfs(p)
total = st.f_blocks * st.f_frsize
free = st.f_bavail * st.f_frsize
used = total - free
pct = round(used / total * 100, 1) if total else 0.0

def human(b):
    for u in ['B','KB','MB','GB','TB']:
        if b < 1024:
            return f'{b:.1f}{u}'
        b /= 1024
    return f'{b:.1f}PB'

print(json.dumps({
    'path': p,
    'total': human(total),
    'used': human(used),
    'free': human(free),
    'percent_used': pct
}))
"
