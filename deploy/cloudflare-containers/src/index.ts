// The Worker in front of the porta container. Every request goes to one named
// instance, so every run's record lives in one place while it is awake. The
// client's Authorization header passes through unchanged; porta checks it.
import { Container, getContainer } from "@cloudflare/containers";

interface Env {
  PORTA: DurableObjectNamespace<PortaContainer>;
  PORTA_JOB_TOKEN: string; // `npx wrangler secret put PORTA_JOB_TOKEN`
}

export class PortaContainer extends Container<Env> {
  defaultPort = 8080;
  // Records and retained output are on the instance's disk, which starts
  // fresh after it sleeps. Keep it awake for the length of an evaluation.
  sleepAfter = "30m";
  // Jobs get no network from porta either way; the container needs none.
  enableInternet = false;

  constructor(ctx: DurableObjectState<{}>, env: Env) {
    super(ctx, env);
    this.envVars = { PORTA_JOB_TOKEN: env.PORTA_JOB_TOKEN };
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    return getContainer(env.PORTA, "porta").fetch(request);
  },
};
