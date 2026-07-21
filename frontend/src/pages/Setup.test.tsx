import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import Setup from './Setup';
import { jsonResponse } from '../test-utils/fetchMocks';

afterEach(() => vi.unstubAllGlobals());

function okFetch() {
  return vi.fn().mockResolvedValue(jsonResponse(undefined));
}

test('defaults to Standalone and posts role + default node id', async () => {
  const fetchMock = okFetch();
  vi.stubGlobal('fetch', fetchMock);
  render(<Setup onSetupComplete={vi.fn()} />);

  fireEvent.change(screen.getByLabelText(/^password$/i), { target: { value: 'pw123456' } });
  fireEvent.change(screen.getByLabelText(/confirm password/i), { target: { value: 'pw123456' } });
  fireEvent.click(screen.getByRole('button', { name: /finish setup/i }));

  await waitFor(() => expect(fetchMock).toHaveBeenCalled());
  const [, init] = fetchMock.mock.calls[0];
  const body = JSON.parse((init as RequestInit).body as string);
  expect(body.role).toBe('standalone');
  expect(body.node_sensor_id).toBe('local');
  expect(body.sensor).toBeUndefined();
});

test('choosing Sensor reveals connection fields and posts them', async () => {
  const fetchMock = okFetch();
  vi.stubGlobal('fetch', fetchMock);
  render(<Setup onSetupComplete={vi.fn()} />);

  fireEvent.click(screen.getByRole('radio', { name: /sensor/i }));

  fireEvent.change(screen.getByLabelText(/^password$/i), { target: { value: 'pw123456' } });
  fireEvent.change(screen.getByLabelText(/confirm password/i), { target: { value: 'pw123456' } });
  fireEvent.change(screen.getByLabelText(/sensor id/i), { target: { value: 'frontgate' } });
  fireEvent.change(screen.getByLabelText(/host/i), { target: { value: 'base.example' } });
  fireEvent.change(screen.getByLabelText(/port/i), { target: { value: '9000' } });
  fireEvent.change(screen.getByLabelText(/encryption key/i), { target: { value: 'a2V5' } });

  fireEvent.click(screen.getByRole('button', { name: /finish setup/i }));

  await waitFor(() => expect(fetchMock).toHaveBeenCalled());
  const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
  expect(body.role).toBe('sensor');
  expect(body.node_sensor_id).toBe('frontgate');
  expect(body.sensor).toMatchObject({ host: 'base.example', port: 9000, key: 'a2V5' });
});

test('Generate fills the encryption key field', async () => {
  vi.stubGlobal('fetch', okFetch());
  render(<Setup onSetupComplete={vi.fn()} />);
  fireEvent.click(screen.getByRole('radio', { name: /sensor/i }));

  const keyField = screen.getByLabelText(/encryption key/i) as HTMLInputElement;
  expect(keyField.value).toBe('');
  fireEvent.click(screen.getByRole('button', { name: /generate/i }));
  expect(keyField.value.length).toBeGreaterThan(0);
});

test('rejects a sensor id containing a space', async () => {
  const fetchMock = okFetch();
  vi.stubGlobal('fetch', fetchMock);
  render(<Setup onSetupComplete={vi.fn()} />);
  fireEvent.click(screen.getByRole('radio', { name: /sensor/i }));
  fireEvent.change(screen.getByLabelText(/^password$/i), { target: { value: 'pw123456' } });
  fireEvent.change(screen.getByLabelText(/confirm password/i), { target: { value: 'pw123456' } });
  fireEvent.change(screen.getByLabelText(/sensor id/i), { target: { value: 'front gate' } });
  fireEvent.click(screen.getByRole('button', { name: /finish setup/i }));

  expect(await screen.findByRole('alert')).toHaveTextContent(/id/i);
  expect(fetchMock).not.toHaveBeenCalled();
});
