#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <sys/signalfd.h>
#include <unistd.h>

int main(void) {
  sigset_t mask;
  sigset_t old_mask;
  assert(sigemptyset(&mask) == 0);
  assert(sigaddset(&mask, SIGUSR1) == 0);
  assert(pthread_sigmask(SIG_BLOCK, &mask, &old_mask) == 0);

  errno = 0;
  assert(signalfd(STDIN_FILENO, &mask, 0) == -1);
  assert(errno == EINVAL);
  errno = 0;
  assert(signalfd(-1, &mask, 1 << 29) == -1);
  assert(errno == EINVAL);

  int fd = signalfd(-1, &mask, SFD_NONBLOCK | SFD_CLOEXEC);
  assert(fd >= 0);
  assert((fcntl(fd, F_GETFL) & O_NONBLOCK) != 0);
  assert((fcntl(fd, F_GETFD) & FD_CLOEXEC) != 0);

  struct signalfd_siginfo info;
  errno = 0;
  assert(read(fd, &info, sizeof(info)) == -1);
  assert(errno == EAGAIN);
  assert(kill(getpid(), SIGUSR1) == 0);

  struct pollfd poll_fd = {.fd = fd, .events = POLLIN};
  assert(poll(&poll_fd, 1, 1000) == 1);
  assert((poll_fd.revents & POLLIN) != 0);
  assert(read(fd, &info, sizeof(info)) == sizeof(info));
  assert(info.ssi_signo == SIGUSR1);
  assert(info.ssi_pid == (unsigned int)getpid());

  assert(close(fd) == 0);
  assert(pthread_sigmask(SIG_SETMASK, &old_mask, NULL) == 0);
  puts("signalfd compatibility tests passed");
  return 0;
}
