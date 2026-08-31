#define _GNU_SOURCE

#include <linux/close_range.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <unistd.h>

#define FD_CACHE_SIZE (1U << 16)

enum fd_kind {
    FD_UNKNOWN,
    FD_OTHER,
    FD_UNIX_SEQPACKET,
};

static unsigned char fd_kinds[FD_CACHE_SIZE];

static void forget_fd(int fd)
{
    if (fd >= 0 && (unsigned int)fd < FD_CACHE_SIZE)
        __atomic_store_n(&fd_kinds[fd], FD_UNKNOWN, __ATOMIC_RELEASE);
}

static int is_unix_seqpacket(int fd)
{
    unsigned char cached;
    int type = -1;
    socklen_t type_length = sizeof(type);
    struct sockaddr_storage address;
    socklen_t address_length = sizeof(address);
    int matches;

    if (fd >= 0 && (unsigned int)fd < FD_CACHE_SIZE) {
        cached = __atomic_load_n(&fd_kinds[fd], __ATOMIC_ACQUIRE);
        if (cached != FD_UNKNOWN)
            return cached == FD_UNIX_SEQPACKET;
    }

    matches = syscall(SYS_getsockopt, fd, SOL_SOCKET, SO_TYPE, &type,
                      &type_length) == 0 && type == SOCK_SEQPACKET &&
        syscall(SYS_getsockname, fd, &address, &address_length) == 0 &&
        address.ss_family == AF_UNIX;

    if (fd >= 0 && (unsigned int)fd < FD_CACHE_SIZE)
        __atomic_store_n(&fd_kinds[fd],
                         matches ? FD_UNIX_SEQPACKET : FD_OTHER,
                         __ATOMIC_RELEASE);
    return matches;
}

ssize_t send(int fd, const void *buffer, size_t length, int flags)
{
    if (is_unix_seqpacket(fd))
        flags |= MSG_EOR;
    return syscall(SYS_sendto, fd, buffer, length, flags, NULL, 0);
}

ssize_t sendto(int fd, const void *buffer, size_t length, int flags,
               const struct sockaddr *address, socklen_t address_length)
{
    if (is_unix_seqpacket(fd))
        flags |= MSG_EOR;
    return syscall(SYS_sendto, fd, buffer, length, flags, address,
                   address_length);
}

ssize_t sendmsg(int fd, const struct msghdr *message, int flags)
{
    if (is_unix_seqpacket(fd))
        flags |= MSG_EOR;
    return syscall(SYS_sendmsg, fd, message, flags);
}

int close(int fd)
{
    forget_fd(fd);
    return syscall(SYS_close, fd);
}

int dup2(int old_fd, int new_fd)
{
    int result = syscall(SYS_dup2, old_fd, new_fd);

    if (result >= 0 && old_fd != new_fd)
        forget_fd(new_fd);
    return result;
}

int dup3(int old_fd, int new_fd, int flags)
{
    int result = syscall(SYS_dup3, old_fd, new_fd, flags);

    if (result >= 0)
        forget_fd(new_fd);
    return result;
}

#ifdef SYS_close_range
int close_range(unsigned int first, unsigned int last, int flags)
{
    unsigned int end;
    unsigned int fd;

    if ((flags & CLOSE_RANGE_CLOEXEC) == 0 && first < FD_CACHE_SIZE) {
        end = last < FD_CACHE_SIZE ? last : FD_CACHE_SIZE - 1;
        for (fd = first; fd <= end; fd++)
            __atomic_store_n(&fd_kinds[fd], FD_UNKNOWN, __ATOMIC_RELEASE);
    }
    return syscall(SYS_close_range, first, last, flags);
}
#endif
