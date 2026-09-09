#!/usr/bin/env bash
set -euo pipefail

mkdir -p pki

# ------------------------------------------------------------
# mkcert CA
# ------------------------------------------------------------

# Creates the local mkcert CA if it doesn't already exist.
mkcert -install

CAROOT="$(mkcert -CAROOT)"

echo "Using mkcert CA: ${CAROOT}"

cp "${CAROOT}/rootCA.pem" pki/ca.crt


# ------------------------------------------------------------
# kube-apiserver server certificate
# ------------------------------------------------------------

mkcert \
  -cert-file pki/apiserver.crt \
  -key-file pki/apiserver.key \
  localhost \
  127.0.0.1 \
  ::1 \
  kube-apiserver \
  kubernetes \
  kubernetes.default \
  kubernetes.default.svc \
  kubernetes.default.svc.cluster.local \
  10.96.0.1


# ------------------------------------------------------------
# Service-account signing key
#
# This is NOT a TLS certificate, so mkcert isn't applicable.
# ------------------------------------------------------------

openssl genrsa \
  -out pki/sa.key \
  4096

openssl rsa \
  -in pki/sa.key \
  -pubout \
  -out pki/sa.pub


# ------------------------------------------------------------
# Kubernetes admin client certificate
#
# Kubernetes x509 authentication interprets:
#   CN = username
#   O  = group
#
# O=system:masters gives cluster-admin privileges.
#
# We sign it using mkcert's CA.
# ------------------------------------------------------------

openssl genrsa \
  -out pki/admin.key \
  4096

openssl req \
  -new \
  -key pki/admin.key \
  -subj "/CN=admin/O=system:masters" \
  -out pki/admin.csr

cat > pki/admin-ext.cnf <<EOF
basicConstraints = CA:FALSE
keyUsage = digitalSignature
extendedKeyUsage = clientAuth
EOF

openssl x509 \
  -req \
  -in pki/admin.csr \
  -CA "${CAROOT}/rootCA.pem" \
  -CAkey "${CAROOT}/rootCA-key.pem" \
  -CAcreateserial \
  -out pki/admin.crt \
  -days 825 \
  -sha256 \
  -extfile pki/admin-ext.cnf

rm -f \
  pki/admin.csr \
  pki/admin-ext.cnf \
  "${CAROOT}/rootCA.srl"

chmod 600 \
  pki/apiserver.key \
  pki/admin.key \
  pki/sa.key

echo
echo "Certificates generated:"
echo "  pki/ca.crt"
echo "  pki/apiserver.crt"
echo "  pki/apiserver.key"
echo "  pki/admin.crt"
echo "  pki/admin.key"
echo "  pki/sa.key"
echo "  pki/sa.pub"