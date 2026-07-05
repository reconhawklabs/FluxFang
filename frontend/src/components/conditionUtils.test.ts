import { expect, test } from 'vitest';
import type { FieldDef } from '../types/catalog';
import {
  adaptValueForOp,
  defaultValueFor,
  firstOpFor,
  isCompleteCondition,
  newConditionFor,
  parseMultiValue,
} from './conditionUtils';

const bssidField: FieldDef = {
  key: 'bssid',
  label: 'BSSID',
  type: 'mac',
  ops: [
    { code: 'eq', label: 'is exactly' },
    { code: 'matches', label: 'contains / matches' },
  ],
};

const channelField: FieldDef = {
  key: 'channel',
  label: 'Channel',
  type: 'number',
  ops: [
    { code: 'eq', label: 'is exactly' },
    { code: 'gte', label: 'is at least' },
    { code: 'lte', label: 'is at most' },
  ],
};

const frameTypeField: FieldDef = {
  key: 'frame_type',
  label: 'Frame type',
  type: 'enum',
  values: ['beacon', 'probe_request'],
  ops: [
    { code: 'eq', label: 'is exactly' },
    { code: 'in', label: 'is any of' },
  ],
};

test('newConditionFor seeds the field, its first op, and that op default value', () => {
  expect(newConditionFor(bssidField)).toEqual({ field: 'bssid', op: 'eq', value: '' });
  expect(newConditionFor(frameTypeField)).toEqual({ field: 'frame_type', op: 'eq', value: 'beacon' });
});

test('firstOpFor returns the first catalog op code', () => {
  expect(firstOpFor(channelField)).toBe('eq');
});

test('defaultValueFor returns [] for the in operator regardless of type', () => {
  expect(defaultValueFor(channelField, 'in')).toEqual([]);
  expect(defaultValueFor(bssidField, 'in')).toEqual([]);
});

test('defaultValueFor returns the first enum value for a non-in op on an enum field', () => {
  expect(defaultValueFor(frameTypeField, 'eq')).toBe('beacon');
});

test('adaptValueForOp wraps a scalar into a single-element array when switching to in', () => {
  expect(adaptValueForOp(channelField, 'in', 6)).toEqual([6]);
  expect(adaptValueForOp(bssidField, 'in', '')).toEqual([]);
});

test('adaptValueForOp unwraps an array to its first element when switching away from in', () => {
  expect(adaptValueForOp(channelField, 'eq', [6, 11])).toBe(6);
  expect(adaptValueForOp(channelField, 'eq', [])).toBe('');
});

test('adaptValueForOp leaves a scalar untouched across a scalar-to-scalar op change', () => {
  expect(adaptValueForOp(bssidField, 'matches', 'aa:bb')).toBe('aa:bb');
});

test('parseMultiValue splits on commas, trims, and drops empties', () => {
  expect(parseMultiValue(' beacon, probe_request ,, ', 'text')).toEqual(['beacon', 'probe_request']);
});

test('parseMultiValue parses number-typed elements and drops non-numeric tokens', () => {
  expect(parseMultiValue('1, 6, abc, 11', 'number')).toEqual([1, 6, 11]);
});

test('isCompleteCondition rejects an unset field/op or empty value/array', () => {
  expect(isCompleteCondition({ field: '', op: 'eq', value: 'x' })).toBe(false);
  expect(isCompleteCondition({ field: 'bssid', op: '', value: 'x' })).toBe(false);
  expect(isCompleteCondition({ field: 'bssid', op: 'eq', value: '' })).toBe(false);
  expect(isCompleteCondition({ field: 'channel', op: 'in', value: [] })).toBe(false);
});

test('isCompleteCondition accepts a fully-specified scalar or non-empty array condition', () => {
  expect(isCompleteCondition({ field: 'bssid', op: 'eq', value: 'aa:bb:cc:dd:ee:ff' })).toBe(true);
  expect(isCompleteCondition({ field: 'channel', op: 'in', value: [1, 6, 11] })).toBe(true);
  expect(isCompleteCondition({ field: 'channel', op: 'eq', value: 0 })).toBe(true);
});
