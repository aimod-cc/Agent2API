import { createRoot } from 'react-dom/client'
import { App } from './App'
import './ui/flclash.css'
import './ui/flclash-controls.css'

const container = document.getElementById('root')
if (container) {
  createRoot(container).render(<App />)
}
