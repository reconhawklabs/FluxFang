import { expect, test } from 'vitest';
import { conditionsToQueryParams } from './filterState';
import type { Condition } from '../types/rule';

test('conditionsToQueryParams emits one repeated cond= per complete condition, dropping incomplete ones', () => {
  const conditions: Condition[] = [
    { field: 'bssid', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' },
    { field: 'channel', op: 'gte', value: 6 },
    { field: '', op: '', value: '' },
  ];

  const params = conditionsToQueryParams(conditions);

  expect(params.getAll('cond')).toEqual(['bssid:eq:"aa:bb:cc:dd:ee:ff"', 'channel:gte:6']);
  expect(params.has('q')).toBe(false);
});

test('conditionsToQueryParams returns an empty params for no conditions', () => {
  expect(conditionsToQueryParams([]).toString()).toBe('');
});
