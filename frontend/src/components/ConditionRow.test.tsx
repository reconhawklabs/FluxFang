// Unit tests for the shared field -> operator -> value machinery
// (`ConditionRow`), independent of `useCatalog`'s fetch — `fields` is passed
// straight in as a prop, so these exercise exactly the dropdown/value-typing
// behavior the Task 9.2 brief's acceptance criteria describe.
import { useState } from 'react';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import type { FieldDef } from '../types/catalog';
import type { Condition } from '../types/rule';
import ConditionRow from './ConditionRow';

const wifiFields: FieldDef[] = [
  {
    key: 'bssid',
    label: 'BSSID',
    type: 'mac',
    ops: [
      { code: 'eq', label: 'is exactly' },
      { code: 'matches', label: 'contains / matches' },
    ],
  },
  {
    key: 'channel',
    label: 'Channel',
    type: 'number',
    ops: [
      { code: 'eq', label: 'is exactly' },
      { code: 'gte', label: 'is at least' },
      { code: 'lte', label: 'is at most' },
    ],
  },
  {
    key: 'frame_type',
    label: 'Frame type',
    type: 'enum',
    values: ['beacon', 'probe_request'],
    ops: [
      { code: 'eq', label: 'is exactly' },
      { code: 'in', label: 'is any of' },
    ],
  },
];

/** A tiny controlled harness so tests can drive multi-step interactions
 * (change field, then change operator, then type a value) against a single
 * piece of state, the same way a real `RuleBuilder`/`FilterBar` parent
 * would re-render `ConditionRow` with the updated `condition` after each
 * `onChange`. */
function Harness({
  initial,
  onChangeSpy,
}: {
  initial: Condition;
  onChangeSpy: (c: Condition) => void;
}) {
  const [condition, setCondition] = useState<Condition>(initial);
  return (
    <ConditionRow
      fields={wifiFields}
      condition={condition}
      index={0}
      onChange={(next) => {
        onChangeSpy(next);
        setCondition(next);
      }}
    />
  );
}

test('operator options come from the catalog and are selectable by their plain-English label, not a typed code', () => {
  render(<ConditionRow fields={wifiFields} condition={{ field: 'bssid', op: 'eq', value: '' }} index={0} onChange={() => {}} />);

  const opSelect = screen.getByLabelText(/operator/i);
  expect(within(opSelect).getByText('is exactly')).toBeInTheDocument();
  expect(within(opSelect).getByText('contains / matches')).toBeInTheDocument();
  // Never a raw op code rendered as user-facing text.
  expect(within(opSelect).queryByText('matches')).not.toBeInTheDocument();
});

test('the field dropdown shows catalog labels, not keys', () => {
  render(<ConditionRow fields={wifiFields} condition={{ field: 'bssid', op: 'eq', value: '' }} index={0} onChange={() => {}} />);

  const fieldSelect = screen.getByLabelText(/field/i);
  expect(within(fieldSelect).getByText('BSSID')).toBeInTheDocument();
  expect(within(fieldSelect).getByText('Channel')).toBeInTheDocument();
  expect(within(fieldSelect).getByText('Frame type')).toBeInTheDocument();
});

test('changing the field resets the operator to a valid one for the new field type', () => {
  const onChangeSpy = vi.fn();
  render(<Harness initial={{ field: 'bssid', op: 'matches', value: 'foo' }} onChangeSpy={onChangeSpy} />);

  fireEvent.change(screen.getByLabelText(/field/i), { target: { value: 'channel' } });

  const lastCall = onChangeSpy.mock.calls.at(-1)![0] as Condition;
  expect(lastCall.field).toBe('channel');
  // 'matches' isn't valid for a number field — must reset to channel's first op.
  expect(lastCall.op).toBe('eq');
  expect(screen.getByLabelText(/operator/i)).toHaveValue('eq');
});

test('selecting a number field\'s gte operator and typing 6 emits the JSON number 6, not the string "6"', () => {
  const onChangeSpy = vi.fn();
  render(<Harness initial={{ field: 'channel', op: 'eq', value: '' }} onChangeSpy={onChangeSpy} />);

  fireEvent.change(screen.getByLabelText(/operator/i), { target: { value: 'gte' } });
  fireEvent.change(screen.getByLabelText(/value/i), { target: { value: '6' } });

  const lastCall = onChangeSpy.mock.calls.at(-1)![0] as Condition;
  expect(lastCall).toEqual({ field: 'channel', op: 'gte', value: 6 });
  expect(typeof lastCall.value).toBe('number');
});

test('switching a number field to the in operator and typing comma-separated values emits a typed number array', () => {
  const onChangeSpy = vi.fn();
  render(<Harness initial={{ field: 'channel', op: 'eq', value: 6 }} onChangeSpy={onChangeSpy} />);

  // channel's catalog fixture above doesn't include `in` — swap to frame_type
  // (which does) via the field dropdown first to exercise the in-operator UI.
  fireEvent.change(screen.getByLabelText(/field/i), { target: { value: 'frame_type' } });
  fireEvent.change(screen.getByLabelText(/operator/i), { target: { value: 'in' } });
  fireEvent.change(screen.getByLabelText(/value/i), { target: { value: 'beacon, probe_request' } });

  const lastCall = onChangeSpy.mock.calls.at(-1)![0] as Condition;
  expect(lastCall).toEqual({ field: 'frame_type', op: 'in', value: ['beacon', 'probe_request'] });
});

test('an enum field renders a select of its catalog values, not a free-text input', () => {
  render(
    <ConditionRow fields={wifiFields} condition={{ field: 'frame_type', op: 'eq', value: 'beacon' }} index={0} onChange={() => {}} />,
  );

  const valueSelect = screen.getByLabelText(/value/i);
  expect(valueSelect.tagName).toBe('SELECT');
  expect(within(valueSelect).getByText('beacon')).toBeInTheDocument();
  expect(within(valueSelect).getByText('probe_request')).toBeInTheDocument();
});

test('a remove button, when provided, calls onRemove', () => {
  const onRemove = vi.fn();
  render(
    <ConditionRow
      fields={wifiFields}
      condition={{ field: 'bssid', op: 'eq', value: '' }}
      index={0}
      onChange={() => {}}
      onRemove={onRemove}
    />,
  );

  fireEvent.click(screen.getByRole('button', { name: /remove/i }));
  expect(onRemove).toHaveBeenCalledTimes(1);
});
