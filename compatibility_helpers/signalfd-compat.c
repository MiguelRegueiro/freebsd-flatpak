#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/signalfd.h>
#include <sys/syscall.h>
#include <unistd.h>

typedef struct {
  int write_fd;
  sigset_t mask;
} SignalFdContext;

static void *signal_waiter(void *data) {
  SignalFdContext *context = data;
  sigset_t write_mask;
  sigemptyset(&write_mask);
  sigaddset(&write_mask, SIGPIPE);
  pthread_sigmask(SIG_BLOCK, &write_mask, NULL);
  for (;;) {
    siginfo_t info;
    int signal_number = sigwaitinfo(&context->mask, &info);
    if (signal_number < 0) {
      if (errno == EINTR)
        continue;
      break;
    }
    struct signalfd_siginfo output;
    memset(&output, 0, sizeof(output));
    output.ssi_signo = (uint32_t)signal_number;
    output.ssi_errno = info.si_errno;
    output.ssi_code = info.si_code;
    output.ssi_pid = (uint32_t)info.si_pid;
    output.ssi_uid = (uint32_t)info.si_uid;
    if (write(context->write_fd, &output, sizeof(output)) != sizeof(output))
      break;
  }
  close(context->write_fd);
  free(context);
  return NULL;
}

int signalfd(int fd, const sigset_t *mask, int flags) {
  int native_fd = (int)syscall(SYS_signalfd4, fd, mask, sizeof(uint64_t), flags);
  if (native_fd >= 0 || errno != ENOSYS)
    return native_fd;
  if (fd != -1 || (flags & ~(SFD_NONBLOCK | SFD_CLOEXEC)) != 0) {
    errno = EINVAL;
    return -1;
  }
  int pipe_fds[2];
  int pipe_flags = 0;
  if (flags & SFD_CLOEXEC)
    pipe_flags |= O_CLOEXEC;
  if (pipe2(pipe_fds, pipe_flags) != 0)
    return -1;
  if ((flags & SFD_NONBLOCK) != 0) {
    int read_flags = fcntl(pipe_fds[0], F_GETFL);
    if (read_flags < 0 || fcntl(pipe_fds[0], F_SETFL,
                                read_flags | O_NONBLOCK) != 0) {
      int saved_errno = errno;
      close(pipe_fds[0]);
      close(pipe_fds[1]);
      errno = saved_errno;
      return -1;
    }
  }

  SignalFdContext *context = calloc(1, sizeof(*context));
  if (context == NULL) {
    close(pipe_fds[0]);
    close(pipe_fds[1]);
    errno = ENOMEM;
    return -1;
  }
  context->write_fd = pipe_fds[1];
  context->mask = *mask;
  sigset_t old_mask;
  int error = pthread_sigmask(SIG_BLOCK, mask, &old_mask);
  pthread_t thread;
  if (error == 0) {
    error = pthread_create(&thread, NULL, signal_waiter, context);
    pthread_sigmask(SIG_SETMASK, &old_mask, NULL);
  }
  if (error != 0) {
    close(pipe_fds[0]);
    close(pipe_fds[1]);
    free(context);
    errno = error;
    return -1;
  }
  pthread_detach(thread);
  return pipe_fds[0];
}
