#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include <linux/types.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/socket.h>
#include <netinet/in.h>

#define NR_create 444
#define NR_add    445
#define NR_self   446
#define LOOPBACK_ADDR   0x7f000001
#define PROBE_DIR_MODE  0700
#define DENIED_PORT     80
#define ALLOWED_PORT    443
#define RULE_PATH_BENEATH 1
#define RULE_NET_PORT     2
#define NET_BIND_TCP    (1ULL << 0)
#define NET_CONNECT_TCP (1ULL << 1)
#define FS_WRITE_FILE   (1ULL << 1)
#define FS_MAKE_REG     (1ULL << 8)

struct ruleset_attr { __u64 fs; __u64 net; };
struct path_beneath  { __u64 allowed; __s32 fd; } __attribute__((packed));
struct net_port      { __u64 allowed; __u64 port; };

static int try_connect(int port) {
    int s = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET; a.sin_port = htons(port);
    a.sin_addr.s_addr = htonl(LOOPBACK_ADDR);
    int r = connect(s, (struct sockaddr *)&a, sizeof(struct sockaddr_in));
    int e = errno; close(s);
    return r == 0 ? 0 : e;
}
static const char *nm(int e) {
    return e == 0 ? "connected" : e == EACCES ? "EACCES (blocked by policy)" : strerror(e);
}
static int try_write(const char *path) {
    int fd = open(path, O_WRONLY | O_CREAT, 0600);
    if (fd < 0) return errno;
    close(fd); return 0;
}

int main(void) {
    long abi = syscall(NR_create, NULL, 0, 1U /* LANDLOCK_CREATE_RULESET_VERSION */);
    if (abi < 0) printf("landlock_abi=unavailable (%s)\n", strerror(errno));
    else printf("landlock_abi=%ld\n", abi);
    mkdir("/tmp/allowed", PROBE_DIR_MODE); mkdir("/tmp/denied", PROBE_DIR_MODE);
    printf("BEFORE  connect:80=%s  connect:443=%s  write /tmp/allowed=%s  write /tmp/denied=%s\n",
           nm(try_connect(DENIED_PORT)), nm(try_connect(ALLOWED_PORT)),
           nm(try_write("/tmp/allowed/a")), nm(try_write("/tmp/denied/a")));

    struct ruleset_attr attr = { .fs = FS_WRITE_FILE | FS_MAKE_REG, .net = NET_CONNECT_TCP };
    int rs = syscall(NR_create, &attr, sizeof(struct ruleset_attr), 0);
    if (rs < 0) { printf("create_ruleset(fs+net) failed: %s\n", strerror(errno)); return 1; }
    printf("ruleset created with fs+net handled\n");

    struct net_port np = { .allowed = NET_CONNECT_TCP, .port = ALLOWED_PORT };
    if (syscall(NR_add, rs, RULE_NET_PORT, &np, 0) < 0)
        printf("add net rule failed: %s\n", strerror(errno));

    int dir = open("/tmp/allowed", O_PATH | O_CLOEXEC);
    struct path_beneath pb = { .allowed = FS_WRITE_FILE | FS_MAKE_REG, .fd = dir };
    if (syscall(NR_add, rs, RULE_PATH_BENEATH, &pb, 0) < 0)
        printf("add path rule failed: %s\n", strerror(errno));

    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)) { perror("no_new_privs"); return 1; }
    if (syscall(NR_self, rs, 0)) { perror("restrict_self"); return 1; }
    printf("restrict_self applied (unprivileged, no namespaces)\n");

    printf("AFTER   connect:80=%s  connect:443=%s  write /tmp/allowed=%s  write /tmp/denied=%s\n",
           nm(try_connect(DENIED_PORT)), nm(try_connect(ALLOWED_PORT)),
           nm(try_write("/tmp/allowed/b")), nm(try_write("/tmp/denied/b")));
    return 0;
}
/*
 * Feasibility probe for docs/roadmap/active/03-linux-native-enforcement.md.
 * Reports what Landlock enforces on the host kernel, unprivileged and without
 * namespaces. Run it on a target before assuming a rule can be applied there:
 *   docker run --rm -v "$PWD:/p" -w /p debian:trixie-slim \
 *     sh -c 'apt-get -qq update && apt-get -qq install -y gcc && \
 *            gcc -O1 -o probe scripts/probes/landlock_probe.c && ./probe'
 * Not part of the build. It is a measurement tool, not an implementation.
 */
