"""Exercise the streamed shell collector with synthetic Linux files/tools."""
import json, os, subprocess, tempfile, unittest
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

class CollectorTests(unittest.TestCase):
    def test_metrics_and_json_escaping(self):
        with tempfile.TemporaryDirectory(prefix='porthop-cockpit-') as directory:
            root = Path(directory)
            (root/'proc/net').mkdir(parents=True)
            (root/'proc/stat').write_text('cpu 10 0 5 80 5 0 0 0 0 0\n')
            (root/'proc/meminfo').write_text('MemTotal: 1000 kB\nMemAvailable: 400 kB\nSwapTotal: 100 kB\nSwapFree: 60 kB\n')
            (root/'proc/uptime').write_text('1234.5 987\n')
            (root/'proc/loadavg').write_text('1.00 2.00 3.00 1/12 55\n')
            (root/'proc/net/dev').write_text('eth0: 1000 1 0 0 0 0 0 0 2000 1 0 0 0 0 0 0\n')
            (root/'os-release').write_text('PRETTY_NAME="Fixture Linux"\n')
            bindir=root/'bin';bindir.mkdir()
            outputs={'ps':' 12 tester 4.2 512 S command"with\\quotes\n',
                     'df':'Filesystem 1-blocks Used Available Capacity Mounted on\n/dev/sda1 100000 40000 50000 44% /mount with spaces\n',
                     'getconf':'8\n', 'uname':'fixture-host\n', 'sleep':''}
            for name,text in outputs.items():
                path=bindir/name
                path.write_text('#!/bin/sh\ncat <<\'FIXTURE\'\n'+text+'FIXTURE\n')
                path.chmod(0o700)
            script=(ROOT/'src-tauri/src/cockpit.sh').read_text().replace('/proc/',str(root/'proc')+'/').replace('/etc/os-release',str(root/'os-release'))
            result=subprocess.run(['sh','-s'],input=script,text=True,capture_output=True,timeout=5,env={**os.environ,'PATH':str(bindir)+':'+os.environ['PATH']})
            self.assertEqual(result.returncode,0,result.stderr)
            data=json.loads(result.stdout)
            self.assertEqual(data['memoryUsed'],600*1024)
            self.assertEqual(data['swapUsed'],40*1024)
            self.assertEqual(data['processes'][0]['name'],'command"with\\quotes')
            self.assertEqual(data['processes'][0]['memory'],512*1024)
            self.assertEqual(data['disks'][0]['mount'],'/mount with spaces')
            self.assertEqual(data['network'][0]['received'],1000)
            self.assertEqual(data['load'],[1,2,3])
            self.assertEqual(data['cpu'],0)
            self.assertEqual(data['processCpuMode'],'lifetime')

unittest.main()
