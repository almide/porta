"""How the escape corpus runs one command under a given sandbox tool.

Every row states its policy in porta's flags, because that is the tool the
corpus was written for. A runner translates those flags into its own tool's
nearest equivalent and returns the argv to run. Where a tool has no
equivalent for part of a row's policy, the runner raises `NotExpressible`, and
the row is reported as "not offered" for that tool rather than as held: a
tool that cannot be asked for a ceiling has not enforced one.

A row that runs with no flags runs each tool with its own defaults. That is a
measurement of what a user gets out of the box. It is not a claim that the
tool cannot be configured to hold the row, and the published table says so.

The translations, per tool:

  porta  the flags as written.
  srt    Anthropic's sandbox-runtime. A settings file: `-v` becomes
         `allowWrite`, strict reads become `denyRead: [$HOME]` with
         `allowRead` for the mounts (the pattern its README gives), and
         `--proxy-allow` becomes `allowedDomains`. srt grants a port only
         with a host, so `--allow-net '*:P'` is written for the one host the
         rows contact, `example.com:P`.
         `--no-net` is an empty `allowedDomains`, srt's own default.
  fence  The same settings shape as srt, with `defaultDenyRead` and
         `allowRead` for strict reads. Fence has no port grants, so a row
         that grants one port is not offered.
  nono   Flags: `-v` becomes `--allow`, `--allow-net '*:P'` becomes
         `--block-net --allow-connect-port P`, `--proxy-allow` becomes
         `--allow-domain`, `--no-net` becomes `--block-net`. nono confines reads by default, so strict reads
         need nothing further. `--max-memory-mb` becomes `--memory`,
         `--max-procs` becomes `--max-processes` (a cgroup `pids.max` on
         Linux); nono has no CPU, file-size or time ceiling. The Python
         interpreter's prefix is granted read
         access, so the probes can start at all, and `--allow-cwd` covers the
         empty directory every tool is started from.
  landrun  Linux only. Landlock with no defaults of its own, so the base
         grants are what any command needs to start, the same set porta's
         strict reads allow: `--rox` for /usr, /bin, /lib and the Python
         prefix, `--ro /etc`, `--rw` for /dev and /tmp. `-v` becomes `--rw`
         (`--ro` for `:ro`), `--allow-net '*:P'` becomes `--connect-tcp P`;
         `--no-net` is landrun's default of no TCP grant.
         landrun targets Landlock ABI 9 and refuses to run on an older kernel
         unless given `--best-effort`, which lets it degrade to what the
         kernel has; it runs that way here, and the table says so. It has no
         host filtering or proxy. A row in proxy mode runs it with no network
         grant at all, which is stricter than a proxy; the one row whose
         point is allowing a host (the cloud metadata row) is not offered.

Every tool is started in the row's first writable mount, or in a fresh empty
directory when the row grants none, so none is handed the caller's working
directory by accident.
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile


class NotExpressible(Exception):
    """The tool has no setting for part of this row's policy."""


def parse_policy(policy):
    """porta's flags, as the fields a translation needs."""
    parsed = {"write": [], "read_only": [], "strict": False, "net": [], "proxy": [], "ceilings": {}, "no_net": False}
    items = list(policy)
    index = 0
    while index < len(items):
        flag, value = items[index], items[index + 1] if index + 1 < len(items) else ""
        index += 2
        if flag == "--no-net":
            parsed["no_net"] = True
            index -= 1
        elif flag == "-v":
            (parsed["read_only"] if value.endswith(":ro") else parsed["write"]).append(value.removesuffix(":ro"))
        elif flag == "--read-policy":
            parsed["strict"] = value == "strict"
        elif flag == "--allow-net":
            parsed["net"].append(value)
        elif flag == "--proxy-allow":
            parsed["proxy"].extend(value.split(","))
        elif flag == "--proxy-audit":
            continue
        elif flag in ("--max-cpu", "--max-procs", "--max-file-size", "--max-memory-mb", "--timeout"):
            parsed["ceilings"][flag] = value
        else:
            raise ValueError(f"the corpus used a flag the runners do not translate: {flag}")
    return parsed


def refuse_ceilings(parsed, tool, offered=()):
    missing = [flag for flag in parsed["ceilings"] if flag not in offered]
    if missing:
        raise NotExpressible(f"{tool} has no {', '.join(missing)}")


class HostNamespaces:
    """For a tool that holds no grant of its own: the host's answer."""

    def gives_pid_namespaces(self, host_gives):
        return host_gives()


class PortaRunner:
    name = "porta"

    def __init__(self, binary):
        self.binary = binary

    def argv(self, target, args, policy):
        # The target is the first bare word before `--`; only its own
        # arguments go after.
        return [self.binary, "run", target, *policy, "--", *args]

    def refused_config(self, result):
        return "porta run <target>" in result.stdout + result.stderr

    def gives_pid_namespaces(self, host_gives):
        # porta may hold a grant the host gives no other program (Ubuntu's
        # AppArmor profile from scripts/apparmor-userns.sh), so it is asked.
        report = subprocess.run([self.binary, "check", "--json"], capture_output=True, text=True)
        primitives = json.loads(report.stdout)["primitives"]
        return any("PID, mount and network namespace" in p["name"] and p["present"] for p in primitives)


class SettingsRunner(HostNamespaces):
    """srt and fence: one settings file per run."""

    def __init__(self, name, binary, separator):
        self.name = name
        self.binary = binary
        self.separator = separator
        self.scratch = pathlib.Path(tempfile.mkdtemp(prefix=f"escapes-{name}-"))
        self.count = 0

    def settings(self, parsed):
        allowed = [self.port_grant(spec) for spec in parsed["net"]] + parsed["proxy"]
        filesystem = {"allowWrite": parsed["write"], "denyWrite": [], "denyRead": [], "allowRead": []}
        if parsed["strict"]:
            if self.name == "fence":
                filesystem["defaultDenyRead"] = True
            else:
                filesystem["denyRead"] = [str(pathlib.Path.home())]
            filesystem["allowRead"] = parsed["write"] + parsed["read_only"]
        return {"network": {"allowedDomains": allowed, "deniedDomains": []}, "filesystem": filesystem}

    def port_grant(self, spec):
        if self.name == "fence":
            raise NotExpressible("fence has no port grants")
        return spec.replace("*:", "example.com:", 1) if spec.startswith("*:") else spec

    def argv(self, target, args, policy):
        parsed = parse_policy(policy)
        refuse_ceilings(parsed, self.name)
        self.count += 1
        path = self.scratch / f"settings-{self.count}.json"
        path.write_text(json.dumps(self.settings(parsed)))
        return [self.binary, "--settings", str(path), *self.separator, target, *args]

    def refused_config(self, result):
        text = result.stdout + result.stderr
        return "does not hold a valid config" in text or "invalid config" in text.lower() or "unknown field" in text


class NonoRunner(HostNamespaces):
    name = "nono"
    # nono's child holds a socket to nono's supervisor (NONO_CAP_FILE and
    # friends name it). That is its design, not a leak, so the
    # descriptor-inheritance row does not score it.
    passes_supervisor_socket = True

    def __init__(self, binary):
        self.binary = binary

    def argv(self, target, args, policy):
        parsed = parse_policy(policy)
        refuse_ceilings(parsed, self.name, offered=("--max-memory-mb", "--max-procs"))
        if parsed["ceilings"] and sys.platform == "darwin":
            # nono's own refusal: "resource limits are only enforced on Linux".
            raise NotExpressible("nono enforces resource limits only on Linux")
        flags = ["run", "--silent", "--no-diagnostics", "--allow-cwd", "--read", sys.base_prefix]
        if "--max-memory-mb" in parsed["ceilings"]:
            flags += ["--memory", parsed["ceilings"]["--max-memory-mb"] + "M"]
        if "--max-procs" in parsed["ceilings"]:
            flags += ["--max-processes", parsed["ceilings"]["--max-procs"]]
        for path in parsed["write"]:
            flags += ["--allow", path]
        for path in parsed["read_only"]:
            flags += ["--read", path]
        flags += self.port_flags(parsed["net"])
        if parsed["no_net"]:
            flags.append("--block-net")
        for host in parsed["proxy"]:
            flags += ["--allow-domain", host]
        return [self.binary, *flags, "--", target, *args]

    @staticmethod
    def port_flags(net):
        ports = [spec.rsplit(":", 1)[1] for spec in net]
        if not ports:
            return []
        if sys.platform == "darwin":
            # nono's own refusal: "Seatbelt cannot filter by TCP port".
            raise NotExpressible("nono cannot filter by TCP port on macOS")
        if "*" in ports:
            raise NotExpressible("nono has no any-port grant under a blocked network")
        return ["--block-net", *[flag for port in ports for flag in ("--allow-connect-port", port)]]

    def refused_config(self, result):
        text = result.stderr
        return "error: unexpected argument" in text or "error: invalid value" in text


METADATA_HOST = "169.254.169.254"


class LandrunRunner(HostNamespaces):
    name = "landrun"

    def __init__(self, binary):
        self.binary = binary

    def argv(self, target, args, policy):
        parsed = parse_policy(policy)
        refuse_ceilings(parsed, self.name)
        if METADATA_HOST in parsed["proxy"]:
            # The one row whose point is a host on the allow-list.
            raise NotExpressible("landrun cannot allow a host, only ports")
        flags = ["--best-effort"]
        for path in ["/usr", "/bin", "/lib", "/lib64", sys.base_prefix]:
            if os.path.exists(path):
                flags += ["--rox", path]
        flags += ["--ro", "/etc", "--rw", "/dev", "--rw", "/tmp"]
        for path in parsed["write"]:
            flags += ["--rw", path]
        for path in parsed["read_only"]:
            flags += ["--ro", path]
        for spec in parsed["net"]:
            port = spec.rsplit(":", 1)[1]
            if port == "*":
                raise NotExpressible("landrun grants ports one at a time")
            flags += ["--connect-tcp", port]
        return [self.binary, *flags, "--", target, *args]

    def refused_config(self, result):
        text = result.stderr
        return "flag provided but not defined" in text or "Incorrect Usage" in text


def make_runner(name, binary):
    if name == "porta":
        return PortaRunner(binary)
    if name == "srt":
        return SettingsRunner("srt", binary, ["--"])
    if name == "fence":
        return SettingsRunner("fence", binary, ["--"])
    if name == "nono":
        return NonoRunner(binary)
    if name == "landrun":
        return LandrunRunner(binary)
    raise SystemExit(f"unknown runner {name}; use porta, srt, fence, nono or landrun")


def environment(extra):
    return {**os.environ, **(extra or {})}
