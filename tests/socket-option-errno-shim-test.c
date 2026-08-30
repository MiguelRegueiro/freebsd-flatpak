#include <errno.h>
#include <stdio.h>
#include <sys/socket.h>

#ifndef SO_PEERPIDFD
#define SO_PEERPIDFD 77
#endif

static int expect_errno(int option_name, int expected)
{
    int value = -1;
    socklen_t length = sizeof(value);

    errno = 0;
    if (getsockopt(-1, SOL_SOCKET, option_name, &value, &length) != -1 ||
        errno != expected) {
        fprintf(stderr, "getsockopt option %d returned errno %d, expected %d\n",
                option_name, errno, expected);
        return 1;
    }
    return 0;
}

int main(void)
{
    if (expect_errno(SO_PEERPIDFD, ENOPROTOOPT) != 0)
        return 1;
    return expect_errno(SO_TYPE, EINVAL);
}
