#!/bin/sh
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
# If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
#
# Copyright (c) 2026 Cristián Rojas Arredondo.
# SPDX-License-Identifier: MPL-2.0

# Genera la CA y el certificado de servidor para 127.0.0.1 del arnés local
# (solo pruebas). Uso: sh make-cert.sh <destino>.
# Escribe ca.pem, server.pem y server-key.pem en el destino.
set -eu

dest="${1:?se necesita el directorio de destino}"
mkdir -p "$dest"

# Configuración mínima propia (portable entre OpenSSL y LibreSSL): la CA firma
# como CA y el servidor solo autentica 127.0.0.1 por SAN de IP.
cat > "$dest/openssl-test.cnf" <<'EOF'
[ req ]
distinguished_name = dn
prompt = no
[ dn ]
CN = avi-bootstrap-test
[ v3_ca ]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
[ v3_server ]
basicConstraints = CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = IP:127.0.0.1
EOF

openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
  -config "$dest/openssl-test.cnf" -extensions v3_ca \
  -subj "/CN=avi-bootstrap-test-ca" \
  -keyout "$dest/ca-key.pem" -out "$dest/ca.pem" 2>/dev/null
openssl req -newkey rsa:2048 -nodes \
  -config "$dest/openssl-test.cnf" -subj "/CN=127.0.0.1" \
  -keyout "$dest/server-key.pem" -out "$dest/server.csr" 2>/dev/null
openssl x509 -req -days 2 -in "$dest/server.csr" \
  -CA "$dest/ca.pem" -CAkey "$dest/ca-key.pem" -CAcreateserial \
  -out "$dest/server.pem" \
  -extfile "$dest/openssl-test.cnf" -extensions v3_server 2>/dev/null
rm -f "$dest/server.csr" "$dest/ca-key.pem" "$dest/ca.srl"
test -s "$dest/ca.pem" || { echo "no se pudo generar la CA" >&2; exit 1; }
test -s "$dest/server.pem" || { echo "no se pudo generar el certificado" >&2; exit 1; }
