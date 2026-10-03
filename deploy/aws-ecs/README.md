# AWS ECS (Fargate)

**Status: template written, not deployed.** Nothing here has run on AWS.

[`task-definition.json`](task-definition.json) runs the evaluation image as
one Fargate task: arm64, 0.5 vCPU / 1 GiB, read-only root filesystem with an
ephemeral volume at `/var/lib/porta` for records and workspaces, the token
from Secrets Manager, logs to CloudWatch, 30 s between SIGTERM and SIGKILL
(porta finishes running jobs as `service_shutdown` within 8 s).

What has to be decided or verified on a real account:

1. **Whether `os_sandbox = "required"` starts.** Fargate's docs say nothing
   about Landlock, seccomp filters a process installs itself, or namespaces
   ([security considerations](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/fargate-security-considerations.html)).
   The service checks at start and refuses with the reason if it cannot. If
   it refuses, switch `command` to `/etc/porta/policy-wasm-only.toml` and
   record that the OS layer is absent there. On ECS **on EC2** you own the
   kernel and the Docker seccomp profile, so `required` can be arranged.
2. **Records survive only as long as the task.** For durable records mount
   EFS at `/var/lib/porta` (`efsVolumeConfiguration`); porta writes records
   with rename, which EFS supports. Not tested.
3. **TLS and access.** porta speaks plain HTTP. Put an ALB with an HTTPS
   listener in front, health check `GET /healthz`, and keep the task's
   security group closed to anything but the ALB.

## Steps (need approval: they create billable resources)

```bash
# 1. Image (arm64 to match the task definition)
docker buildx build --platform linux/arm64 -t <ACCOUNT_ID>.dkr.ecr.<REGION>.amazonaws.com/porta-eval:<TAG> --push .
# 2. Token
aws secretsmanager create-secret --name porta-eval-token --secret-string "$(openssl rand -hex 24)"
# 3. Task definition and a service in an existing cluster/VPC
aws ecs register-task-definition --cli-input-json file://deploy/aws-ecs/task-definition.json
aws ecs create-service --cluster <CLUSTER> --service-name porta-eval --task-definition porta-eval \
  --desired-count 1 --launch-type FARGATE \
  --network-configuration 'awsvpcConfiguration={subnets=[<SUBNET>],securityGroups=[<SG>],assignPublicIp=DISABLED}'
# 4. Evaluate through a tunnel or the ALB
PORTA_JOB_TOKEN=... python3 examples/enterprise/evaluate.py --url https://<ALB>
# Remove
aws ecs delete-service --cluster <CLUSTER> --service porta-eval --force
aws ecs deregister-task-definition --task-definition porta-eval:1
aws secretsmanager delete-secret --secret-id porta-eval-token --force-delete-without-recovery
```

The execution role needs `AmazonECSTaskExecutionRolePolicy` and
`secretsmanager:GetSecretValue` on the token. The task needs no task role:
porta calls no AWS API.
