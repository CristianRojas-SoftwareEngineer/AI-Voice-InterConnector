#!/usr/bin/env python3
"""Servidor falso local para las suites del bootstrap POSIX (solo pruebas).

Sirve el directorio dado por HTTPS (o HTTP sin certificado) en 127.0.0.1 con
un puerto efímero, que comunica por el fichero de puerto. Registra cada
petición en el fichero de registro para las aserciones de "antes de descargar".
Solo biblioteca estándar, sin dependencias.
"""

import argparse
import http.server
import os
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, fmt, *args):
        line = "%s %s\n" % (self.address_string(), (fmt % args))
        with open(self.server.request_log, "a", encoding="utf-8") as handle:
            handle.write(line)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dir", required=True)
    parser.add_argument("--port-file", required=True)
    parser.add_argument("--request-log", required=True)
    parser.add_argument("--cert", default="")
    parser.add_argument("--key", default="")
    args = parser.parse_args()
    os.chdir(args.dir)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.request_log = args.request_log
    if args.cert:
        import ssl

        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(args.cert, args.key or None)
        server.socket = context.wrap_socket(server.socket, server_side=True)
    with open(args.port_file, "w", encoding="utf-8") as handle:
        handle.write(str(server.server_address[1]))
    server.serve_forever()


main()
