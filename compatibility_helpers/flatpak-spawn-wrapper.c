#define _GNU_SOURCE

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#ifndef REAL_FLATPAK_SPAWN
#define REAL_FLATPAK_SPAWN "/usr/bin/flatpak-spawn"
#endif
#ifndef SIGNALFD_PRELOAD
#define SIGNALFD_PRELOAD "/run/host/freebsd-flatpak/libsignalfd-shim.so"
#endif

int main(int argc, char **argv)
{
    const char *old_preload = getenv("LD_PRELOAD");
    char *new_preload;
    const char *trace = getenv("FREEBSD_FLATPAK_TRACE_LINUX_COMPAT");

    if (trace != NULL && *trace != '\0') {
        fputs("freebsd-flatpak: flatpak-spawn", stderr);
        for (int i = 1; i < argc; i++)
            fprintf(stderr, " %s", argv[i]);
        fputc('\n', stderr);
    }

    (void)argc;
    if (old_preload != NULL && *old_preload != '\0') {
        size_t shim_length = strlen(SIGNALFD_PRELOAD);
        size_t old_length = strlen(old_preload);

        if (old_length > SIZE_MAX - shim_length - 2) {
            errno = ENOMEM;
            perror("flatpak-spawn compatibility wrapper");
            return 127;
        }
        new_preload = malloc(shim_length + old_length + 2);
        if (new_preload == NULL) {
            perror("flatpak-spawn compatibility wrapper");
            return 127;
        }
        memcpy(new_preload, SIGNALFD_PRELOAD, shim_length);
        new_preload[shim_length] = ':';
        memcpy(new_preload + shim_length + 1, old_preload, old_length + 1);
    } else {
        new_preload = strdup(SIGNALFD_PRELOAD);
        if (new_preload == NULL) {
            perror("flatpak-spawn compatibility wrapper");
            return 127;
        }
    }

    if (setenv("LD_PRELOAD", new_preload, 1) < 0) {
        perror("flatpak-spawn compatibility wrapper");
        free(new_preload);
        return 127;
    }
    free(new_preload);
    execv(REAL_FLATPAK_SPAWN, argv);
    perror("exec " REAL_FLATPAK_SPAWN);
    return 127;
}
