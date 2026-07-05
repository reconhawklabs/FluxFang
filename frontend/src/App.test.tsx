import { render, screen, waitFor } from '@testing-library/react'
import { vi } from 'vitest'
import App from './App'

test('renders backend health status', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
    ok: true, json: async () => ({ status: 'ok' }),
  }))
  render(<App />)
  await waitFor(() => expect(screen.getByText(/backend: ok/i)).toBeInTheDocument())
})
