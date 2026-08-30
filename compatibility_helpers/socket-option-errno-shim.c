#define _GNU_SOURCE

#include <errno.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef SO_PEERPIDFD
#define SO_PEERPIDFD 77
#endif

static int raw_getsockopt(int fd, int level, int option_name,
                          void *option_value, socklen_t *option_length)
{
#ifdef SOCKET_OPTION_ERRNO_SHIM_TEST
    (void)fd;
    (void)level;
    (void)option_name;
    (void)option_value;
    (void)option_length;
    errno = EINVAL;
    return -1;
#else
    return (int)syscall(SYS_getsockopt, fd, level, option_name, option_value,
                        option_length);
#endif
}

int getsockopt(int fd, int level, int option_name, void *option_value,
               socklen_t *option_length)
{
    int result = raw_getsockopt(fd, level, option_name, option_value,
                                option_length);

    if (result < 0 && errno == EINVAL && level == SOL_SOCKET &&
        option_name == SO_PEERPIDFD)
        errno = ENOPROTOOPT;
    return result;
}
