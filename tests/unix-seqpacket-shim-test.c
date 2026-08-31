#define _GNU_SOURCE

#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <unistd.h>

static int socket_type(int fd)
{
    int type = -1;
    socklen_t length = sizeof(type);

    if (getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &length) != 0) {
        perror("getsockopt(SO_TYPE)");
        return -1;
    }
    return type;
}

static int expect_record(int fd, const char *expected, size_t expected_length)
{
    char buffer[64];
    ssize_t length = recv(fd, buffer, sizeof(buffer), 0);

    if (length != (ssize_t)expected_length ||
        memcmp(buffer, expected, expected_length) != 0) {
        fprintf(stderr, "received %zd bytes, expected %zu\n", length,
                expected_length);
        return 1;
    }
    return 0;
}

static int create_tcp_pair(int pair[2])
{
    struct sockaddr_in address = {
        .sin_family = AF_INET,
        .sin_addr.s_addr = htonl(INADDR_LOOPBACK),
    };
    socklen_t address_length = sizeof(address);
    int listener;

    listener = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (listener < 0 ||
        bind(listener, (const struct sockaddr *)&address, sizeof(address)) !=
            0 ||
        getsockname(listener, (struct sockaddr *)&address, &address_length) !=
            0 ||
        listen(listener, 1) != 0) {
        perror("create TCP listener");
        return 1;
    }

    pair[0] = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (pair[0] < 0 ||
        connect(pair[0], (const struct sockaddr *)&address, sizeof(address)) !=
            0) {
        perror("connect TCP socket");
        close(listener);
        return 1;
    }
    pair[1] = accept4(listener, NULL, NULL, SOCK_CLOEXEC);
    close(listener);
    if (pair[1] < 0) {
        perror("accept TCP socket");
        close(pair[0]);
        return 1;
    }
    return 0;
}

int main(void)
{
    int pair[2];
    int datagram_pair[2];
    int stream_pair[2];
    int tcp_pair[2];
    char first[40];
    char second[48];
    const char third_a[] = "third ";
    const char third_b[] = "record";
    const struct iovec third[] = {
        { .iov_base = (void *)third_a, .iov_len = sizeof(third_a) - 1 },
        { .iov_base = (void *)third_b, .iov_len = sizeof(third_b) },
    };
    struct msghdr message = {
        .msg_iov = (struct iovec *)third,
        .msg_iovlen = 2,
    };

    memset(first, 'A', sizeof(first));
    memset(second, 'B', sizeof(second));

    if (socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0, pair) != 0) {
        perror("socketpair(SOCK_SEQPACKET)");
        return 1;
    }
    if (socket_type(pair[0]) != SOCK_SEQPACKET ||
        (fcntl(pair[0], F_GETFD) & FD_CLOEXEC) == 0) {
        fprintf(stderr, "SOCK_SEQPACKET identity or flags changed\n");
        return 1;
    }
    if (send(pair[0], first, sizeof(first), 0) != (ssize_t)sizeof(first) ||
        send(pair[0], second, sizeof(second), 0) != (ssize_t)sizeof(second) ||
        sendmsg(pair[0], &message, 0) !=
            (ssize_t)(sizeof(third_a) - 1 + sizeof(third_b))) {
        perror("send records");
        return 1;
    }
    if (expect_record(pair[1], first, sizeof(first)) != 0 ||
        expect_record(pair[1], second, sizeof(second)) != 0 ||
        expect_record(pair[1], "third record", sizeof("third record")) != 0)
        return 1;

    if (socketpair(AF_UNIX, SOCK_DGRAM, 0, datagram_pair) != 0 ||
        socket_type(datagram_pair[0]) != SOCK_DGRAM) {
        fprintf(stderr, "ordinary SOCK_DGRAM socketpair changed\n");
        return 1;
    }
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, stream_pair) != 0 ||
        socket_type(stream_pair[0]) != SOCK_STREAM) {
        fprintf(stderr, "ordinary SOCK_STREAM socketpair changed\n");
        return 1;
    }

    if (create_tcp_pair(tcp_pair) != 0)
        return 1;
    if (dup2(tcp_pair[0], pair[0]) != pair[0]) {
        perror("replace cached SOCK_SEQPACKET fd");
        return 1;
    }
    close(tcp_pair[0]);
    {
        char byte;
        if (recv(pair[1], &byte, sizeof(byte), 0) != 0) {
            fprintf(stderr, "SOCK_SEQPACKET EOF semantics changed\n");
            return 1;
        }
        if (send(pair[0], "x", 1, 0) != 1 ||
            recv(tcp_pair[1], &byte, sizeof(byte), 0) != 1 || byte != 'x') {
            fprintf(stderr, "fd classification cache was not invalidated\n");
            return 1;
        }
    }
    close(pair[0]);
    close(pair[1]);
    close(tcp_pair[1]);
    close(datagram_pair[0]);
    close(datagram_pair[1]);
    close(stream_pair[0]);
    close(stream_pair[1]);
    return 0;
}
