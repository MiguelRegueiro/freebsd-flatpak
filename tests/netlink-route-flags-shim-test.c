#include <assert.h>
#include <errno.h>
#include <linux/if.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>

ssize_t recv(int fd, void *buffer, size_t length, int flags);

static unsigned char reply[NLMSG_SPACE(sizeof(struct ifinfomsg))];
static size_t reply_length;
static int socket_domain;
static int socket_protocol;

ssize_t netlink_route_flags_shim_test_real_recv(int fd, void *buffer,
                                                 size_t length, int flags)
{
    (void)fd;
    (void)flags;
    assert(length >= reply_length);
    memcpy(buffer, reply, reply_length);
    return (ssize_t)reply_length;
}

int netlink_route_flags_shim_test_getsockopt(int fd, int level, int option,
                                              void *value, socklen_t *length)
{
    int result;
    (void)fd;
    assert(level == SOL_SOCKET);
    assert(*length == sizeof(result));
    if (option == SO_DOMAIN) {
        result = socket_domain;
    } else {
        assert(option == SO_PROTOCOL);
        result = socket_protocol;
    }
    memcpy(value, &result, sizeof(result));
    *length = sizeof(result);
    return 0;
}

static struct ifinfomsg *prepare_link(unsigned int flags)
{
    struct nlmsghdr *header = (struct nlmsghdr *)reply;
    struct ifinfomsg *link;

    memset(reply, 0, sizeof(reply));
    header->nlmsg_len = NLMSG_LENGTH(sizeof(*link));
    header->nlmsg_type = RTM_NEWLINK;
    reply_length = header->nlmsg_len;
    link = NLMSG_DATA(header);
    link->ifi_flags = flags;
    return link;
}

static void active_route_link_gains_lower_up(void)
{
    unsigned char received[sizeof(reply)];
    struct ifinfomsg *link = prepare_link(IFF_UP | IFF_RUNNING);
    socket_domain = AF_NETLINK;
    socket_protocol = NETLINK_ROUTE;

    assert(recv(7, received, sizeof(received), 0) == (ssize_t)reply_length);
    link = NLMSG_DATA((struct nlmsghdr *)received);
    assert(link->ifi_flags & IFF_LOWER_UP);
}

static void down_link_is_untouched(void)
{
    unsigned char received[sizeof(reply)];
    struct ifinfomsg *link = prepare_link(IFF_UP);
    socket_domain = AF_NETLINK;
    socket_protocol = NETLINK_ROUTE;

    assert(recv(8, received, sizeof(received), 0) == (ssize_t)reply_length);
    link = NLMSG_DATA((struct nlmsghdr *)received);
    assert(!(link->ifi_flags & IFF_LOWER_UP));
}

static void unrelated_socket_is_untouched(void)
{
    unsigned char received[sizeof(reply)];
    struct ifinfomsg *link = prepare_link(IFF_UP | IFF_RUNNING);
    socket_domain = AF_INET;
    socket_protocol = 0;

    assert(recv(9, received, sizeof(received), 0) == (ssize_t)reply_length);
    link = NLMSG_DATA((struct nlmsghdr *)received);
    assert(!(link->ifi_flags & IFF_LOWER_UP));
}

int main(void)
{
    active_route_link_gains_lower_up();
    down_link_is_untouched();
    unrelated_socket_is_untouched();
    puts("netlink route flags shim tests passed");
    return 0;
}
