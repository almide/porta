# Azure Container Apps

**Status: template written, not deployed.** Nothing here has run on Azure.

[`containerapp.yaml`](containerapp.yaml) runs the evaluation image as one
replica with internal ingress on port 8080, the token from Key Vault through
the app's system identity, `GET /healthz` as the startup probe.

Constraints ([containers](https://learn.microsoft.com/en-us/azure/container-apps/containers),
[storage](https://learn.microsoft.com/en-us/azure/container-apps/storage-mounts),
[lifecycle](https://learn.microsoft.com/en-us/azure/container-apps/application-lifecycle-management)):

- **amd64 image.** Build with `--platform linux/amd64`.
- **The OS sandbox is unverified.** Azure documents no privileged containers
  and nothing about seccomp, Landlock or namespaces for ordinary apps. If
  `policy.toml` refuses to start, use `/etc/porta/policy-wasm-only.toml`.
- **Records are ephemeral** (1–8 GiB by vCPU). Durable records need an
  Azure Files mount at `/var/lib/porta`; porta's rename-based writes over SMB
  are untested.
- **Ingress times out at 240 s** by default; keep `?wait=` below it.
- **SIGTERM, then 30 s.**
- The system identity needs `AcrPull` on the registry and
  `Key Vault Secrets User` on the secret before the first revision starts.

## Steps (need approval: they create billable resources)

```bash
docker buildx build --platform linux/amd64 -t <REGISTRY>.azurecr.io/porta-eval:<TAG> --push .
az keyvault secret set --vault-name <VAULT> -n porta-eval-token --value "$(openssl rand -hex 24)"
az containerapp create -n porta-eval -g <RESOURCE_GROUP> --yaml deploy/azure-container-apps/containerapp.yaml
# Remove
az containerapp delete -n porta-eval -g <RESOURCE_GROUP>
```
