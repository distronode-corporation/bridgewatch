# Test-only TLS certificate

`untrusted-cert.pem` and `untrusted-key.pem` are a self-signed certificate for `127.0.0.1` and
its key, made for `../crash_report_tls.rs` and used nowhere else. The test serves them from a
local TLS server so that it can check a client refuses a certificate no system trusts. They
protect nothing and must never be trusted by anything.

Made with:

```
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 36500 \
  -subj "/CN=bridgewatch-test-only" -addext "subjectAltName=IP:127.0.0.1" \
  -addext "basicConstraints=critical,CA:FALSE" \
  -keyout untrusted-key.pem -out untrusted-cert.pem
```
