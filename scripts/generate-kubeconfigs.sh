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
  for environment in local compose; do
    if [[ "$environment" == local ]]; then
      config_path="pki/${identity}.kubeconfig"
      server="https://127.0.0.1:6443"
      certificate_dir="${root_dir}/pki"
    else
      config_path="pki/${identity}.compose.kubeconfig"
      server="https://kube-apiserver:6443"
      certificate_dir="/etc/kubernetes/pki"
    fi
    cat > "$config_path" <<CONFIG
apiVersion: v1
kind: Config
clusters:
  - name: suntitle-local
    cluster:
      server: ${server}
      certificate-authority: ${certificate_dir}/ca.crt
users:
  - name: ${identity}
    user:
      client-certificate: ${certificate_dir}/${identity}.crt
      client-key: ${certificate_dir}/${identity}.key
contexts:
  - name: suntitle-local
    context:
      cluster: suntitle-local
      user: ${identity}
      namespace: suntitle
current-context: suntitle-local
CONFIG
  done
done
