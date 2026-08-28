#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/signalfd.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

struct emulated_signalfd {
    int read_fd;
    int write_fd;
    bool closed;
    sigset_t mask;
    pthread_t thread;
    pthread_mutex_t lock;
    struct emulated_signalfd *next;
};

static pthread_mutex_t states_lock = PTHREAD_MUTEX_INITIALIZER;
static struct emulated_signalfd *states;

static int raw_close(int fd)
{
    return (int)syscall(SYS_close, fd);
}

static int set_fd_flag(int fd, int command, int flag)
{
    int current = fcntl(fd, command);
    if (current < 0)
        return -1;
    return fcntl(fd, command == F_GETFD ? F_SETFD : F_SETFL, current | flag);
}

static void fill_signalfd_info(struct signalfd_siginfo *output,
                               const siginfo_t *input)
{
    memset(output, 0, sizeof(*output));
    output->ssi_signo = (uint32_t)input->si_signo;
    output->ssi_errno = input->si_errno;
    output->ssi_code = input->si_code;
    output->ssi_pid = (uint32_t)input->si_pid;
    output->ssi_uid = (uint32_t)input->si_uid;
    output->ssi_fd = input->si_fd;
    output->ssi_tid = (uint32_t)input->si_timerid;
    output->ssi_band = (uint32_t)input->si_band;
    output->ssi_overrun = (uint32_t)input->si_overrun;
    output->ssi_status = input->si_status;
    output->ssi_int = input->si_int;
    output->ssi_ptr = (uint64_t)(uintptr_t)input->si_ptr;
    output->ssi_addr = (uint64_t)(uintptr_t)input->si_addr;
}

static void *signal_waiter(void *opaque)
{
    struct emulated_signalfd *state = opaque;

    for (;;) {
        sigset_t mask;
        struct timespec timeout = {.tv_sec = 0, .tv_nsec = 10000000};
        siginfo_t info;
        int signal_number;
        int write_fd;

        pthread_mutex_lock(&state->lock);
        if (state->closed) {
            pthread_mutex_unlock(&state->lock);
            return NULL;
        }
        mask = state->mask;
        write_fd = state->write_fd;
        pthread_mutex_unlock(&state->lock);

        signal_number = sigtimedwait(&mask, &info, &timeout);
        if (signal_number < 0) {
            if (errno == EAGAIN || errno == EINTR)
                continue;
            continue;
        }

        struct signalfd_siginfo output;
        fill_signalfd_info(&output, &info);
        ssize_t written = write(write_fd, &output, sizeof(output));
        if (written < 0 && (errno == EBADF || errno == EPIPE))
            return NULL;
    }
}

static struct emulated_signalfd *find_state_locked(int fd)
{
    for (struct emulated_signalfd *state = states; state != NULL;
         state = state->next) {
        if (state->read_fd == fd)
            return state;
    }
    return NULL;
}

static bool remove_state_locked(struct emulated_signalfd *target)
{
    struct emulated_signalfd **link;

    for (link = &states; *link != NULL; link = &(*link)->next) {
        if (*link == target) {
            *link = target->next;
            target->next = NULL;
            return true;
        }
    }
    return false;
}

static int update_emulated_signalfd(struct emulated_signalfd *state,
                                    const sigset_t *mask)
{
    pthread_mutex_lock(&state->lock);
    state->mask = *mask;
    pthread_mutex_unlock(&state->lock);
    return state->read_fd;
}

static int create_emulated_signalfd(const sigset_t *mask, int flags)
{
    int pipe_fds[2];
    struct emulated_signalfd *state;
    int saved_errno;

    if (pipe(pipe_fds) < 0)
        return -1;
    if (set_fd_flag(pipe_fds[1], F_GETFD, FD_CLOEXEC) < 0 ||
        set_fd_flag(pipe_fds[1], F_GETFL, O_NONBLOCK) < 0 ||
        ((flags & SFD_CLOEXEC) != 0 && set_fd_flag(pipe_fds[0], F_GETFD, FD_CLOEXEC) < 0) ||
        ((flags & SFD_NONBLOCK) != 0 &&
         set_fd_flag(pipe_fds[0], F_GETFL, O_NONBLOCK) < 0)) {
        saved_errno = errno;
        raw_close(pipe_fds[0]);
        raw_close(pipe_fds[1]);
        errno = saved_errno;
        return -1;
    }

    state = calloc(1, sizeof(*state));
    if (state == NULL) {
        saved_errno = errno;
        raw_close(pipe_fds[0]);
        raw_close(pipe_fds[1]);
        errno = saved_errno;
        return -1;
    }
    state->read_fd = pipe_fds[0];
    state->write_fd = pipe_fds[1];
    state->mask = *mask;
    pthread_mutex_init(&state->lock, NULL);

    pthread_mutex_lock(&states_lock);
    state->next = states;
    states = state;
    pthread_mutex_unlock(&states_lock);

    int error = pthread_create(&state->thread, NULL, signal_waiter, state);
    if (error != 0) {
        pthread_mutex_lock(&states_lock);
        (void)remove_state_locked(state);
        pthread_mutex_unlock(&states_lock);
        raw_close(pipe_fds[0]);
        raw_close(pipe_fds[1]);
        pthread_mutex_destroy(&state->lock);
        free(state);
        errno = error;
        return -1;
    }
    return pipe_fds[0];
}

int signalfd(int fd, const sigset_t *mask, int flags)
{
    struct emulated_signalfd *state;
    long result;

    if ((flags & ~(SFD_CLOEXEC | SFD_NONBLOCK)) != 0) {
        errno = EINVAL;
        return -1;
    }

    pthread_mutex_lock(&states_lock);
    state = find_state_locked(fd);
    if (state != NULL) {
        int updated = update_emulated_signalfd(state, mask);
        pthread_mutex_unlock(&states_lock);
        return updated;
    }
    pthread_mutex_unlock(&states_lock);

    result = syscall(SYS_signalfd4, fd, mask, _NSIG / 8, flags);
    if (result >= 0 || errno != ENOSYS || fd >= 0)
        return (int)result;
    return create_emulated_signalfd(mask, flags);
}

int close(int fd)
{
    struct emulated_signalfd *state = NULL;

    pthread_mutex_lock(&states_lock);
    for (state = states; state != NULL; state = state->next) {
        if (state->read_fd == fd) {
            (void)remove_state_locked(state);
            break;
        }
    }
    pthread_mutex_unlock(&states_lock);

    if (state != NULL) {
        pthread_mutex_lock(&state->lock);
        state->closed = true;
        pthread_mutex_unlock(&state->lock);
        (void)pthread_join(state->thread, NULL);
        raw_close(state->write_fd);
        pthread_mutex_destroy(&state->lock);
        free(state);
    }
    return raw_close(fd);
}

#ifdef SIGNALFD_SHIM_TEST
size_t signalfd_shim_state_count_for_test(void)
{
    size_t count = 0;

    pthread_mutex_lock(&states_lock);
    for (struct emulated_signalfd *state = states; state != NULL;
         state = state->next)
        count++;
    pthread_mutex_unlock(&states_lock);
    return count;
}
#endif
