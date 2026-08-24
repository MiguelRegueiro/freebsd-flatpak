#include "document_id.h"
#include "document_grant_store.h"

static char *encode_document_id(const guint8 *bytes, gsize length) {
  char *encoded = g_base64_encode(bytes, length);
  for (char *p = encoded; *p != '\0'; p++) {
    if (*p == '=') {
      *p = '\0';
      break;
    }
    if (*p == '+' || *p == '/') {
      *p = '_';
    }
  }
  return encoded;
}

char *generate_document_id(BridgeState *state) {
  for (;;) {
    guint32 random_words[4];
    for (gsize i = 0; i < G_N_ELEMENTS(random_words); i++) {
      random_words[i] = g_random_int();
    }
    char *candidate = encode_document_id((const guint8 *)random_words,
                                         sizeof(random_words));
    if (find_grant(state, candidate) == NULL) {
      return candidate;
    }
    g_free(candidate);
  }
}

bool document_id_is_valid(const char *doc_id) {
  if (doc_id == NULL || *doc_id == '\0' || g_strcmp0(doc_id, ".") == 0 ||
      g_strcmp0(doc_id, "..") == 0) {
    return false;
  }
  for (const char *p = doc_id; *p != '\0'; p++) {
    if (!g_ascii_isalnum(*p) && *p != '_' && *p != '-' && *p != '.') {
      return false;
    }
  }
  return true;
}
