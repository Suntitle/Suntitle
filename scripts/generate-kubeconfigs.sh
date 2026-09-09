#!/usr/bin/env bash
# Run from the repository root after generate-certs.sh. Existing keys are retained.
set -euo pipefail
umask 077
root_dir="$(pwd)"
ca_dir="$(mkcert -CAROOT)"
for identity in suntitle-api suntitle-proxy; do
  if [[ ! -f "pki/${identity}.crt" || ! -f "pki/${identity}.key" ]]; then
    openssl req -new -newkey rsa:3072 -nodes \
      -keyout "pki/${identity}.key" -subj "/CN=${identity}" \
      -out "pki/${identity}.csr"
    openssl x509 -req -in "pki/${identity}.csr" \
      -CA "${ca_dir}/rootCA.pem" -CAkey "${ca_dir}/rootCA-key.pem" \
      -set_serial "0x$(openssl rand -hex 16)" -days 365 -sha256 \
      -extfile <(printf '%s\n' 'basicConstraints=CA:FALSE' 'keyUsage=digitalSignature' 'extendedKeyUsage=clientAuth') \
      -out "pki/${identity}.crt"
    rm "pki/${identity}.csr"
  fi
done
for identity in admin suntitle-api suntitle-proxy; do
  cat > "pki/${identity}.kubeconfig" <<CONFIG
apiVersion: v1
kind: Config
clusters:
  - name: suntitle-local
    cluster:
      server: https://127.0.0.1:6443
      certificate-authority: ${root_dir}/pki/ca.crt
users:
  - name: ${identity}
    user:
      client-certificate: ${root_dir}/pki/${identity}.crt
      client-key: ${root_dir}/pki/${identity}.key
contexts:
  - name: suntitle-local
    context:
      cluster: suntitle-local
      user: ${identity}
      namespace: suntitle
current-context: suntitle-local
CONFIG
done
