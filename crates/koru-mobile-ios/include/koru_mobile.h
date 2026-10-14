/* SPDX-License-Identifier: MPL-2.0 */

/* Canonical Protobuf C ABI for the generated koru.api.v1 mobile contract.
 * Every operation accepts its corresponding serialized request and returns
 * its serialized response. A null pointer means failure; a successful empty
 * response has a non-null pointer and length zero. The caller frees every
 * response exactly once. */

#ifndef KORU_MOBILE_H
#define KORU_MOBILE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct KoruBuffer {
    uint8_t *ptr;
    size_t len;
} KoruBuffer;

KoruBuffer koru_api_status(const uint8_t *request, size_t len);
KoruBuffer koru_api_link_device(const uint8_t *request, size_t len);
KoruBuffer koru_api_advertise_build(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_inventory(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_document(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_blob(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_blob_status(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_cloud_sync_status(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_cloud_waiting(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_set_blob_policy(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_request_blob(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_file_blob(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_export_blob(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_release_blobs(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_recover_blobs(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_blob_arrivals(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_run(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_run_timeline(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_mutate_workspace(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_mutate_profile(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_mutate_document(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_import_file(const uint8_t *request, size_t len);
KoruBuffer koru_api_store_ensure_file_note(const uint8_t *request, size_t len);
KoruBuffer koru_api_start_pairing(const uint8_t *request, size_t len);
KoruBuffer koru_api_cancel_pairing(const uint8_t *request, size_t len);
KoruBuffer koru_api_devices(const uint8_t *request, size_t len);
KoruBuffer koru_api_device_history(const uint8_t *request, size_t len);
KoruBuffer koru_api_unlink_device(const uint8_t *request, size_t len);
KoruBuffer koru_api_rename_device(const uint8_t *request, size_t len);
KoruBuffer koru_api_reset_identity(const uint8_t *request, size_t len);
KoruBuffer koru_api_set_relay(const uint8_t *request, size_t len);
KoruBuffer koru_api_account_command(const uint8_t *request, size_t len);
KoruBuffer koru_api_account_poll(const uint8_t *request, size_t len);
KoruBuffer koru_api_account_platform(const uint8_t *request, size_t len);

void koru_buffer_free(KoruBuffer buffer);

#ifdef __cplusplus
}
#endif

#endif
