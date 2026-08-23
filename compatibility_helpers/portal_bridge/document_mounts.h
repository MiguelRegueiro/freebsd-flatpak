#ifndef DOCUMENT_MOUNTS_H
#define DOCUMENT_MOUNTS_H
#include "portal_bridge_process.h"
bool run_argv(char **, GError **);
bool mount_file_read_only(const char *, const char *, GError **);
bool unmount_path(const char *);
#endif
