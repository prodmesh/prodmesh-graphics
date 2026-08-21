/**
 * Signing in, as a band of chrome under the header. ADR 0025.
 *
 * Not a modal, because ADR 0020 says nothing floats — and here that rule earns
 * its keep rather than merely being obeyed. A refusal can arrive mid-edit: an
 * overlay that steals focus, or a route that replaces the workspace, would
 * unmount an unpublished Template to ask for a PIN. This pushes the console
 * down by 48px, takes the credentials, and hands the operator back exactly the
 * screen they were on.
 */

import { useEffect, useRef, useState } from 'react'
import { LockKeyhole, X } from 'lucide-react'

import { signIn, type Denial, type Me } from '../lib/identity'

export function SignInBar({
  me,
  denial,
  onDone,
  onDismiss,
}: {
  me: Me | null
  /** The authority a refused action wanted, when that is why the bar opened. */
  denial: Denial | null
  onDone: () => void
  onDismiss: () => void
}) {
  const [username, setUsername] = useState('')
  const [pin, setPin] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const first = useRef<HTMLInputElement>(null)

  useEffect(() => first.current?.focus(), [])

  // Somebody already signed in is not short of a session, they are short of an
  // authority — and the fix is a different account, not the same one again.
  const shortOfAuthority = Boolean(denial?.permission && me?.user)

  const submit = async () => {
    setBusy(true)
    setError('')
    const r = await signIn(username.trim(), pin)
    setBusy(false)
    if (!r.ok) {
      setPin('')
      setError(r.label)
      return
    }
    onDone()
  }

  return (
    <div className="c-signin" role="region" aria-label="Sign in">
      <LockKeyhole size={15} className="c-signin__mark" aria-hidden />

      <p className="c-signin__why">
        {shortOfAuthority ? (
          <>
            <strong>{me?.user?.username}</strong> can’t: {denial?.label?.replace(/^You don’t have permission to:\s*/, '') ?? denial?.permission}
            {' '}Sign in as someone who can.
          </>
        ) : denial?.permission ? (
          <>Sign in to do that.</>
        ) : (
          <>This station can watch. Sign in to change anything.</>
        )}
      </p>

      <form
        className="c-signin__form"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <input
          ref={first}
          className="c-signin__field"
          aria-label="Username"
          autoComplete="username"
          placeholder="Username"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
        />
        <input
          className="c-signin__field c-signin__field--pin"
          aria-label="PIN"
          type="password"
          inputMode="numeric"
          autoComplete="current-password"
          placeholder="PIN"
          value={pin}
          onChange={(e) => setPin(e.target.value)}
        />
        {/* Secondary, not primary: primary is red, red is ON AIR and only ever
            ON AIR (ADR 0020). Signing in is not a take. */}
        <button
          className="c-btn c-btn--secondary"
          type="submit"
          disabled={busy || !username.trim() || !pin}
        >
          Sign in
        </button>
      </form>

      {/* A failure is a sentence, never a colour (ADR 0020). */}
      {error ? (
        <span className="c-signin__error" role="status">
          {error}
        </span>
      ) : null}

      <button className="c-signin__close" onClick={onDismiss} aria-label="Dismiss sign in">
        <X size={15} />
      </button>
    </div>
  )
}
