#define _GNU_SOURCE

#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <linux/if.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <pthread.h>
#include <string.h>
#include <sys/socket.h>

typedef ssize_t (*recv_fn)(int fd, void *buffer, size_t length, int flags);

#ifdef NETLINK_ROUTE_FLAGS_SHIM_TEST

extern ssize_t netlink_route_flags_shim_test_real_recv(int fd, void *buffer,
                                                        size_t length,
                                                        int flags);
extern int netlink_route_flags_shim_test_getsockopt(int fd, int level,
                                                     int option, void *value,
                                                     socklen_t *length);

static ssize_t call_real_recv(int fd, void *buffer, size_t length, int flags)
{
    return netlink_route_flags_shim_test_real_recv(fd, buffer, length, flags);
}

static int call_getsockopt(int fd, int level, int option, void *value,
                           socklen_t *length)
{
    return netlink_route_flags_shim_test_getsockopt(fd, level, option, value,
                                                     length);
}

#else

static pthread_once_t resolve_recv_once = PTHREAD_ONCE_INIT;
static recv_fn real_recv;

static void resolve_recv(void)
{
    void *symbol = dlsym(RTLD_NEXT, "recv");
    memcpy(&real_recv, &symbol, sizeof(symbol));
}

static ssize_t call_real_recv(int fd, void *buffer, size_t length, int flags)
{
    pthread_once(&resolve_recv_once, resolve_recv);
    if (real_recv == NULL) {
        errno = ENOSYS;
        return -1;
    }
    return real_recv(fd, buffer, length, flags);
}

static int call_getsockopt(int fd, int level, int option, void *value,
                           socklen_t *length)
{
    return getsockopt(fd, level, option, value, length);
}

#endif

static int is_route_netlink_socket(int fd)
{
    int domain;
    int protocol;
    socklen_t length = sizeof(domain);

    if (call_getsockopt(fd, SOL_SOCKET, SO_DOMAIN, &domain, &length) != 0 ||
        length != sizeof(domain) || domain != AF_NETLINK) {
        return 0;
    }
    length = sizeof(protocol);
    return call_getsockopt(fd, SOL_SOCKET, SO_PROTOCOL, &protocol, &length) ==
               0 &&
           length == sizeof(protocol) && protocol == NETLINK_ROUTE;
}

static void add_lower_up_to_running_links(void *buffer, size_t length)
{
    struct nlmsghdr *header;
    int remaining;

    if (length > (size_t)INT_MAX) {
        return;
    }
    remaining = (int)length;
    for (header = buffer; NLMSG_OK(header, remaining);
         header = NLMSG_NEXT(header, remaining)) {
        struct ifinfomsg *link;

        if (header->nlmsg_type != RTM_NEWLINK ||
            header->nlmsg_len < NLMSG_LENGTH(sizeof(*link))) {
            continue;
        }
        link = NLMSG_DATA(header);
        if (!(link->ifi_flags & IFF_LOOPBACK) &&
            (link->ifi_flags & IFF_UP) && (link->ifi_flags & IFF_RUNNING)) {
            link->ifi_flags |= IFF_LOWER_UP;
        }
    }
}

ssize_t recv(int fd, void *buffer, size_t length, int flags)
{
    ssize_t result = call_real_recv(fd, buffer, length, flags);

    if (result > 0 && is_route_netlink_socket(fd)) {
        add_lower_up_to_running_links(buffer, (size_t)result);
    }
    return result;
}
