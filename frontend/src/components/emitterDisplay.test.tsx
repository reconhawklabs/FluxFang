import { describe, expect, test } from 'vitest';
import { render, screen } from '@testing-library/react';
import {
  MAC_PERSISTENCE_FILTER_OPTIONS,
  MacIdentityCell,
  macPersistenceBadge,
} from './emitterDisplay';
import type { Emitter } from '../api/emitters';

/** Minimal `Emitter` carrying just the fields `MacIdentityCell` reads. */
function emitterWith(attributes: Record<string, unknown>): Emitter {
  return {
    id: 'e1',
    name: 'test',
    attributes,
  } as unknown as Emitter;
}

describe('macPersistenceBadge', () => {
  test('groups the two long-lived classes under "randomized-longterm"', () => {
    expect(macPersistenceBadge({ mac_persistence: 'per_network' })).toBe(
      'randomized-longterm',
    );
    expect(macPersistenceBadge({ mac_persistence: 'session' })).toBe(
      'randomized-longterm',
    );
  });

  test('badges the short-lived classes as plain "randomized"', () => {
    expect(macPersistenceBadge({ mac_persistence: 'ephemeral' })).toBe('randomized');
    expect(macPersistenceBadge({ mac_persistence: 'unlinkable' })).toBe('randomized');
  });

  test('shows no badge for a stable (non-randomized) address', () => {
    expect(macPersistenceBadge({ mac_persistence: 'stable' })).toBeNull();
  });

  test('falls back to the legacy boolean for pre-classification emitters', () => {
    // Emitters classified before `mac_persistence` existed can't be
    // resolved any finer than "randomized".
    expect(macPersistenceBadge({ randomized_mac: true })).toBe('randomized');
    expect(macPersistenceBadge({ randomized_mac: false })).toBeNull();
    expect(macPersistenceBadge({})).toBeNull();
  });

  test('prefers the class over a stale legacy boolean', () => {
    expect(
      macPersistenceBadge({ mac_persistence: 'session', randomized_mac: true }),
    ).toBe('randomized-longterm');
  });
});

describe('MacIdentityCell', () => {
  test('renders a static-random BLE address as long-term, not throwaway', () => {
    render(
      <MacIdentityCell
        emitter={emitterWith({
          address: 'db:e5:df:32:9a:aa',
          mac_persistence: 'session',
          randomized_mac: true,
        })}
      />,
    );
    expect(screen.getByTestId('emitter-randomized-badge-e1')).toHaveTextContent(
      'randomized-longterm',
    );
  });

  test('renders a probe-request MAC as plain randomized', () => {
    render(
      <MacIdentityCell
        emitter={emitterWith({
          src_mac: '3a:de:ad:be:ef:00',
          mac_persistence: 'ephemeral',
          randomized_mac: true,
        })}
      />,
    );
    expect(screen.getByTestId('emitter-randomized-badge-e1')).toHaveTextContent(
      'randomized',
    );
  });

  test('renders no badge for a vendor MAC', () => {
    render(
      <MacIdentityCell
        emitter={emitterWith({
          bssid: '00:11:22:33:44:55',
          mac_persistence: 'stable',
        })}
      />,
    );
    expect(screen.queryByTestId('emitter-randomized-badge-e1')).not.toBeInTheDocument();
  });
});

describe('MAC_PERSISTENCE_FILTER_OPTIONS', () => {
  test('offers exactly the tokens the backend accepts', () => {
    // Must stay in step with
    // `fluxfang_core::classify::PERSISTENCE_FILTER_TOKENS` — anything else
    // is a 400, not an empty result.
    expect(MAC_PERSISTENCE_FILTER_OPTIONS.map((o) => o.value)).toEqual([
      'randomized',
      'randomized-longterm',
      'stable',
      'per_network',
      'session',
      'ephemeral',
      'unlinkable',
    ]);
  });
});
