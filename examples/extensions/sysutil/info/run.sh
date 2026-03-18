#!/usr/bin/env bash
python3 -c "
import json, sys, os, platform, subprocess
print(json.dumps({
    'os': platform.system(),
    'os_version': platform.release(),
    'architecture': platform.machine(),
    'hostname': platform.node(),
    'cwd': os.getcwd(),
    'user': os.environ.get('USER', ''),
    'rust_version': subprocess.run(
        ['rustc', '--version'], capture_output=True, text=True
    ).stdout.strip() if os.popen('which rustc').read().strip() else 'N/A'
}))
"
