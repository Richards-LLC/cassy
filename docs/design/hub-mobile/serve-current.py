"""Serve the committed Commander dist under its production /commander/ mount."""
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import argparse

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--port', type=int, default=8417)
args = parser.parse_args()
root = Path(__file__).resolve().parents[3] / 'hub-web' / 'dist'


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(root), **kwargs)

    def do_GET(self):
        if self.path.startswith('/commander/'):
            self.path = self.path[len('/commander'):]
        super().do_GET()


ThreadingHTTPServer(('127.0.0.1', args.port), Handler).serve_forever()
