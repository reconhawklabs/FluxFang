-- Backfill `emitter.attributes.mac_persistence` for emitters classified
-- before the persistence class existed (see
-- `fluxfang_core::classify::MacPersistence`).
--
-- No schema change: the class lives in the existing `attributes` JSONB, and
-- the per-data-source retention settings live in `data_source.config`. This
-- migration exists purely so an existing installation's *historical*
-- emitters answer the new `mac_persistence=` filters and show the right
-- badge, instead of the feature only working for captures taken after the
-- upgrade.
--
-- Additive and idempotent: every statement only writes rows where the key
-- is still absent, and only ever adds a key -- no attribute is removed or
-- overwritten. Emitters whose address is missing or malformed are skipped
-- (the regex guard), leaving them exactly as they are today: they fall back
-- to the legacy `randomized_mac` boolean for their badge, match no
-- persistence filter, and stay exempt from the ephemeral age-out sweep.

-- ---------------------------------------------------------------------
-- Bluetooth: the subtype of a *random* address is the top two bits of the
-- first octet (11 static random, 01 resolvable private, 00 non-resolvable;
-- 10 is reserved and claims the least). This is deliberately NOT the
-- locally-administered bit -- `c0:...` is static random with the LA bit
-- clear -- and the top-two-bits test is only applied when the controller
-- reported the address as random, since a public OUI such as `c8:9f:bb`
-- can coincidentally start with those bits.
--
-- When `address_type` was never recorded the subtype is unknowable, so this
-- mirrors `bluetooth_persistence`'s fallback: LA bit set -> the
-- conservative `ephemeral`, else `stable`.
-- ---------------------------------------------------------------------
UPDATE emitter SET attributes = attributes || jsonb_build_object(
    'mac_persistence',
    CASE
        WHEN attributes->>'address_type' = 'random' THEN
            CASE get_byte(decode(substr(attributes->>'address', 1, 2), 'hex'), 0) >> 6
                WHEN 3 THEN 'session'      -- static random: until reboot
                WHEN 1 THEN 'ephemeral'    -- resolvable private: ~15 min
                WHEN 0 THEN 'unlinkable'   -- non-resolvable private
                ELSE 'ephemeral'           -- 10 is reserved by the spec
            END
        WHEN attributes->>'address_type' IS NOT NULL THEN 'stable'
        WHEN (get_byte(decode(substr(attributes->>'address', 1, 2), 'hex'), 0) & 2) <> 0
            THEN 'ephemeral'
        ELSE 'stable'
    END)
WHERE emitter_type = 'bluetooth_device'
  AND attributes->>'mac_persistence' IS NULL
  AND attributes->>'address' ~* '^[0-9a-f]{2}:';

-- ---------------------------------------------------------------------
-- Wi-Fi access points: always `stable`, matching `wifi_persistence`'s
-- beacon arm. The locally-administered bit on a BSSID means "secondary
-- virtual interface" -- one radio broadcasting several SSIDs -- not privacy
-- randomization, and that BSSID is fixed for the life of the AP's config.
-- Reading the LA bit here would badge 40% of ordinary infrastructure as
-- randomized (measured on a dense urban capture: 1,780 of 4,489 APs).
-- ---------------------------------------------------------------------
UPDATE emitter SET attributes = attributes || jsonb_build_object(
    'mac_persistence', 'stable')
WHERE emitter_type = 'wifi_access_point'
  AND attributes->>'mac_persistence' IS NULL
  AND attributes->>'bssid' ~* '^[0-9a-f]{2}:';

-- ---------------------------------------------------------------------
-- Wi-Fi clients: the frame type is what separates the two randomized
-- lifetimes, and it lives on the *emissions*, not on the emitter -- the
-- same locally-administered MAC is throwaway in a probe request but
-- per-network in an association. So this looks for evidence the client
-- ever actually associated:
--   * `connected_bssid`, which `classify_wifi_association` seeds; or
--   * any attached (re)association emission.
-- Either means the address is the one the client connects with, which
-- persists per network for months. With no such evidence the only frames
-- seen were probes, so the address is throwaway.
--
-- A non-randomized src_mac is `stable` regardless, and is checked first so
-- the emission scan is skipped for it.
-- ---------------------------------------------------------------------
UPDATE emitter e SET attributes = e.attributes || jsonb_build_object(
    'mac_persistence',
    CASE
        WHEN (get_byte(decode(substr(e.attributes->>'src_mac', 1, 2), 'hex'), 0) & 2) = 0
            THEN 'stable'
        WHEN e.attributes->>'connected_bssid' IS NOT NULL THEN 'per_network'
        WHEN EXISTS (
            SELECT 1 FROM emission em
            WHERE em.emitter_id = e.id
              AND em.payload->>'frame_type' IN ('association_request', 'reassociation_request')
        ) THEN 'per_network'
        ELSE 'ephemeral'
    END)
WHERE e.emitter_type = 'wifi_client'
  AND e.attributes->>'mac_persistence' IS NULL
  AND e.attributes->>'src_mac' ~* '^[0-9a-f]{2}:';

-- TPMS sensors are deliberately left alone: a sensor id isn't a MAC and is
-- never randomized, so it has no class. `MacRetention::should_store` treats
-- a classless emitter as always-retained, and the age-out sweep only
-- targets `ephemeral`, so both stay correct with the key absent.
