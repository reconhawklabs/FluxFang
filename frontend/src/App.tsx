import { useEffect, useState } from 'react'

export default function App() {
  const [status, setStatus] = useState('…')
  useEffect(() => {
    fetch('/api/health').then(r => r.json()).then(d => setStatus(d.status)).catch(() => setStatus('down'))
  }, [])
  return <div className="p-8 text-lg">FluxFang — backend: {status}</div>
}
