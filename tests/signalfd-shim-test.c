#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/signalfd.h>
#include <unistd.h>

size_t signalfd_shim_state_count_for_test(void);

static int expect_signal(int fd, int expected)
{
    struct pollfd poll_fd = {.fd = fd, .events = POLLIN};
    struct signalfd_siginfo info;

    if (kill(getpid(), expected) < 0) {
        perror("kill");
        return 1;
    }
    if (poll(&poll_fd, 1, 2000) != 1) {
        perror("poll");
        return 1;
    }
    if (read(fd, &info, sizeof(info)) != (ssize_t)sizeof(info)) {
        perror("read");
        return 1;
    }
    if (info.ssi_signo != (uint32_t)expected ||
        info.ssi_pid != (uint32_t)getpid() ||
        info.ssi_uid != (uint32_t)getuid()) {
        fprintf(stderr, "unexpected signalfd record: signal=%u pid=%u uid=%u\n",
                info.ssi_signo, info.ssi_pid, info.ssi_uid);
        return 1;
    }
    return 0;
}

static int stress_close_and_fd_reuse(const sigset_t *mask)
{
    for (int iteration = 0; iteration < 200; iteration++) {
        int fd = signalfd(-1, mask, SFD_CLOEXEC | SFD_NONBLOCK);
        if (fd < 0) {
            perror("stress signalfd create");
            return 1;
        }
        if (kill(getpid(), SIGUSR1) < 0) {
            perror("stress kill");
            return 1;
        }
        if (close(fd) < 0) {
            perror("stress close");
            return 1;
        }
        if (signalfd_shim_state_count_for_test() != 0) {
            fprintf(stderr, "signalfd state leaked after close\n");
            return 1;
        }

        int reused[2];
        unsigned char bytes[sizeof(struct signalfd_siginfo) + 1];
        if (pipe(reused) < 0) {
            perror("stress pipe");
            return 1;
        }
        if (reused[0] != fd) {
            fprintf(stderr, "closed signalfd read fd was not reused\n");
            return 1;
        }
        if (write(reused[1], "x", 1) != 1) {
            perror("stress pipe write");
            return 1;
        }
        ssize_t count = read(reused[0], bytes, sizeof(bytes));
        if (count != 1 || bytes[0] != 'x') {
            fprintf(stderr, "closed signalfd worker wrote to a reused fd\n");
            return 1;
        }
        close(reused[0]);
        close(reused[1]);
    }
    return 0;
}

int main(void)
{
    sigset_t blocked;
    sigset_t mask;
    int fd;

    sigemptyset(&blocked);
    sigaddset(&blocked, SIGUSR1);
    sigaddset(&blocked, SIGUSR2);
    if (sigprocmask(SIG_BLOCK, &blocked, NULL) < 0) {
        perror("sigprocmask");
        return 1;
    }

    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
    fd = signalfd(-1, &mask, SFD_CLOEXEC | SFD_NONBLOCK);
    if (fd < 0) {
        perror("signalfd create");
        return 1;
    }
    if ((fcntl(fd, F_GETFD) & FD_CLOEXEC) == 0 ||
        (fcntl(fd, F_GETFL) & O_NONBLOCK) == 0) {
        fprintf(stderr, "signalfd flags were not applied\n");
        return 1;
    }
    if (expect_signal(fd, SIGUSR1) != 0)
        return 1;

    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR2);
    if (signalfd(fd, &mask, 0) != fd) {
        perror("signalfd update");
        return 1;
    }
    if (expect_signal(fd, SIGUSR2) != 0)
        return 1;

    if (close(fd) < 0) {
        perror("close");
        return 1;
    }
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
    if (stress_close_and_fd_reuse(&mask) != 0)
        return 1;
    return 0;
}
