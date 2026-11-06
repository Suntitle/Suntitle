#!/usr/bin/env bash
# Generate credentials on the instance. Existing CA and client identities MUST
# be retained while their Kubernetes data persists. Run from the deployment root.
set -euo pipefail
umask 077
mkdir -p pki
mkdir -p private-ca

if [[ ! -s pki/ca.crt ]]; then
    openssl req -x509 -newkey rsa:3072 -nodes -sha256 -days 3650 \
        -subj '/CN=Suntitle Kubernetes CA' -keyout private-ca/ca.key -out pki/ca.crt
fi
if [[ ! -s pki/apiserver.crt ]]; then
    openssl req -new -newkey rsa:3072 -nodes -subj '/CN=kube-apiserver' \
        -keyout pki/apiserver.key -out pki/apiserver.csr
    printf 'subjectAltName=DNS:kube-apiserver,DNS:localhost,DNS:kubernetes,DNS:kubernetes.default,DNS:kubernetes.default.svc,DNS:kubernetes.default.svc.cluster.local,IP:127.0.0.1,IP:10.96.0.1\nextendedKeyUsage=serverAuth\n' > pki/server.ext
    openssl x509 -req -in pki/apiserver.csr -CA pki/ca.crt -CAkey private-ca/ca.key \
        -set_serial "0x$(openssl rand -hex 16)" -days 825 -sha256 \
        -extfile pki/server.ext -out pki/apiserver.crt
fi
if [[ ! -s pki/sa.key ]]; then
    openssl genrsa -out pki/sa.key 4096
    openssl rsa -in pki/sa.key -pubout -out pki/sa.pub
fi
for identity in admin suntitle-api suntitle-proxy; do
    if [[ ! -s "pki/${identity}.crt" ]]; then
        subject="/CN=${identity}"
        if [[ "$identity" == admin ]]; then subject+='/O=system:masters'; fi
        openssl req -new -newkey rsa:3072 -nodes -subj "$subject" \
            -keyout "pki/${identity}.key" -out "pki/${identity}.csr"
        printf 'basicConstraints=CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=clientAuth\n' > pki/client.ext
        openssl x509 -req -in "pki/${identity}.csr" -CA pki/ca.crt -CAkey private-ca/ca.key \
            -set_serial "0x$(openssl rand -hex 16)" -days 825 -sha256 \
            -extfile pki/client.ext -out "pki/${identity}.crt"
    fi
    cat > "pki/${identity}.compose.kubeconfig" <<CONFIG
apiVersion: v1
kind: Config
clusters:
  - name: suntitle
    cluster:
      server: https://kube-apiserver:6443
      certificate-authority: /etc/kubernetes/pki/ca.crt
users:
  - name: ${identity}
    user:
      client-certificate: /etc/kubernetes/pki/${identity}.crt
      client-key: /etc/kubernetes/pki/${identity}.key
contexts:
  - name: suntitle
    context:
      cluster: suntitle
      user: ${identity}
      namespace: suntitle
current-context: suntitle
CONFIG
done
chmod 600 pki/*.key
