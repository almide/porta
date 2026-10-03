# Google Cloud Run

**Status: template written, not deployed.** Nothing here has run on Google Cloud.

[`service.yaml`](service.yaml) runs the evaluation image as a Cloud Run
service on the **gen2** execution environment, one instance at most, the
token from Secret Manager, `GET /healthz` as the startup probe.

Constraints that follow from the platform
([container contract](https://docs.cloud.google.com/run/docs/container-contract),
[execution environments](https://docs.cloud.google.com/run/docs/about-execution-environments)):

- **amd64 image.** Build with `--platform linux/amd64`.
- **Pin gen2.** gen1 is gVisor; whether Landlock and porta's seccomp filter
  work there is undocumented, and porta refuses `os_sandbox = "required"`
  where they do not. On gen2 it is also undocumented — verify; if the service
  refuses to start, use `/etc/porta/policy-wasm-only.toml`.
- **Records are in memory.** The writable filesystem is in-memory, counts
  against the instance's memory and disappears with the instance. One
  instance (`maxScale: 1`) keeps every record in one place while it lives;
  across instances a client would see a 404 for a run another instance ran.
  Durable records need a Cloud Storage FUSE or NFS volume at
  `/var/lib/porta` (gen2 only; FUSE has no file locking — porta does not need
  locks, but this is untested).
- **Requests end at `timeoutSeconds`.** `?wait=` must stay below it.
- **SIGTERM, then 10 s.** porta stops running jobs within 8 s.

## Steps (need approval: they create billable resources)

```bash
gcloud artifacts repositories create porta --repository-format docker --location <REGION>
docker buildx build --platform linux/amd64 -t <REGION>-docker.pkg.dev/<PROJECT>/porta/porta-eval:<TAG> --push .
printf '%s' "$(openssl rand -hex 24)" | gcloud secrets create porta-eval-token --data-file=-
gcloud run services replace deploy/gcp-cloud-run/service.yaml --region <REGION>
# grant roles/secretmanager.secretAccessor on the secret to the service's identity
PORTA_JOB_TOKEN=... python3 examples/enterprise/evaluate.py --url https://<SERVICE_URL>
# Remove
gcloud run services delete porta-eval --region <REGION>
gcloud secrets delete porta-eval-token
gcloud artifacts repositories delete porta --location <REGION>
```

`ingress: internal-and-cloud-load-balancing` keeps the service off the public
internet; reach it from inside the VPC or through a load balancer, or change
it for an evaluation you accept being public (the token still applies).
