# MAI Update Protocol

This protocol describes the optional online model update path. MAI systems must
continue to work fully air-gapped; update checks send no device identifier,
profile identifier, analytics payload, cookies, or telemetry.

## Endpoints

All endpoints use HTTPS.

### `GET /v1/updates/manifest?tier=scout&version=1.0.0`

Returns models available to a product tier and MAI software version.

```json
{
  "season": "2026-summer",
  "models": [
    {
      "name": "qwen3-14b",
      "version": "1.1.0",
      "size": 8123456789,
      "url": "https://mirror.example/v1/updates/packages/qwen3-14b/1.1.0/manifest",
      "tier": "scout"
    }
  ]
}
```

Manifest checks require no authentication.

### `GET /v1/updates/packages/{name}/{version}/manifest`

Returns package metadata and weight shards.

```json
{
  "name": "qwen3-14b",
  "version": "1.1.0",
  "license_tier": "scout",
  "shards": [
    {
      "name": "weights-00001.bin",
      "size": 104857600,
      "hash": "blake3:...",
      "url": "https://mirror.example/v1/updates/packages/qwen3-14b/1.1.0/weights/weights-00001.bin"
    }
  ]
}
```

### `GET /v1/updates/packages/{name}/{version}/weights/{shard}`

Returns raw shard bytes. Servers should support `Range` requests and return
`206 Partial Content` for resumable downloads.

## Tiers

`scout`: 2-3 small single-GPU models per seasonal update.

`ranger`: 5-7 models, including larger and multi-GPU models.

`pack_leader`: complete library.

Seasonal cadence is four releases per year. Paid package downloads may require
a license key, but manifest checks remain unauthenticated.

## License Validation

License validation is entitlement-based:

- A higher tier includes lower tier packages.
- Expired licenses must be rejected.
- Offline packages embed entitlement metadata in the signed package manifest.
- Clients must not phone home to validate an offline package.

## Differential Updates

Clients compare local shard hashes with the package manifest. Matching shards
are reused; changed or missing shards are downloaded. Downloads must go to a
temporary location first, then be verified and handed to the Session 24 install
pipeline.

## Privacy Rules

Update clients may send only:

- requested tier
- current MAI software version
- package name/version/shard path
- optional license key for paid package bytes

They must not send serial numbers, hardware IDs, profile IDs, request history,
adapter telemetry, cookies, or analytics events.
