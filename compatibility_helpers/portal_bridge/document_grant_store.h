#ifndef DOCUMENT_GRANT_STORE_H
#define DOCUMENT_GRANT_STORE_H
#include "portal_bridge_process.h"
GVariant *path_bytes_variant(const char *);
void free_grant(DocumentGrant *);
char **read_permissions(void);
char **read_write_permissions(void);
void add_mountpoint_extra(BridgeState *, GVariantBuilder *);
bool create_document_grant_from_path(BridgeState *, const char *, const char *,
                                     char **, bool, bool, bool,
                                     DocumentGrant **, GError **);
bool create_document_grant_from_fd(BridgeState *, int, const char *, GVariant *,
                                   bool, bool, bool, DocumentGrant **,
                                   GError **);
bool restore_document_grant(BridgeState *, const char *, const char *,
                            const char *, char **, bool, DocumentGrant **,
                            GError **);
bool register_document_grant(BridgeState *, DocumentGrant *, GError **);
void merge_document_permissions(DocumentGrant *, char **);
DocumentGrant *find_grant(BridgeState *, const char *);
DocumentGrant *find_reusable_grant(BridgeState *, const char *, bool, bool);
char *sandbox_uri_for_grant(BridgeState *, DocumentGrant *);
char *host_path_for_document_path(BridgeState *, const char *);
#endif
