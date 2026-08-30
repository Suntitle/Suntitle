import { useEffect, useState, type FormEvent } from "react"
import { Box, Eye, EyeOff, Languages } from "lucide-react"
import { observer } from "mobx-react-lite"
import { HostsPage } from "@/pages/hosts-page"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Checkbox } from "@/components/ui/checkbox"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Separator } from "@/components/ui/separator"
import { Spinner } from "@/components/ui/spinner"
import { useStores } from "@/stores"

function AuthenticationCheckScreen() {
  return (
    <main className="grid min-h-svh place-items-center bg-violet-50/70 px-6" aria-busy="true">
      <div className="flex flex-col items-center gap-8 text-center">
        <div className="flex items-center gap-2.5">
          <span className="grid size-10 place-items-center rounded-lg bg-violet-600 text-white">
            <Box className="size-[18px]" aria-hidden="true" />
          </span>
          <span className="text-xl font-semibold tracking-tight text-foreground">Suntitle</span>
        </div>

        <div className="flex flex-col items-center gap-3" aria-live="polite">
          <Spinner className="size-6 text-violet-600" />
          <div className="grid gap-2">
            <h1 className="text-2xl font-semibold tracking-tight">Checking your session…</h1>
            <p className="text-sm text-muted-foreground">Securely confirming your access to Suntitle.</p>
          </div>
        </div>
      </div>
    </main>
  )
}

const App = observer(function App() {
  const [showPassword, setShowPassword] = useState(false)
  const { authStore } = useStores()

  useEffect(() => {
    void authStore.initialize()
  }, [authStore])

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const form = new FormData(event.currentTarget)

    try {
      await authStore.login({
        username: String(form.get("username") ?? ""),
        password: String(form.get("password") ?? ""),
      })
    } catch {
      // AuthStore exposes the normalized API error to this observer.
    }
  }

  if (!authStore.isInitialized) return <AuthenticationCheckScreen />
  if (authStore.isAuthenticated) return <HostsPage />

  return (
    <main className="grid min-h-svh bg-background lg:grid-cols-2">
      <section className="flex min-h-svh flex-col p-6 sm:p-10 lg:p-12">
        <a href="/" className="flex w-fit items-center gap-2.5 font-semibold tracking-tight text-foreground">
          <span className="grid size-8 place-items-center rounded-lg bg-primary text-primary-foreground"><Box className="size-4" /></span>
          Suntitle
        </a>

        <div className="m-auto w-full max-w-[400px] py-12">
          <Card className="border-0 shadow-none">
            <CardHeader className="px-0 pt-0">
              <p className="mb-2 text-xs font-semibold uppercase tracking-[0.14em] text-muted-foreground">Welcome back</p>
              <CardTitle className="text-3xl tracking-[-0.04em] sm:text-4xl">Sign in to your account</CardTitle>
              <CardDescription className="pt-2 leading-6">Enter your details below to continue to your workspace.</CardDescription>
            </CardHeader>
            <CardContent className="px-0 pb-0">
              <Button variant="outline" className="h-10 w-full">
                <svg viewBox="0 0 24 24" className="size-4" aria-hidden="true"><path fill="#4285f4" d="M21.6 12.2c0-.7-.1-1.4-.2-2H12v3.9h5.4a4.6 4.6 0 0 1-2 3v2.6h3.2c1.9-1.8 3-4.4 3-7.5Z"/><path fill="#34a853" d="M12 22c2.7 0 5-.9 6.6-2.4l-3.2-2.5c-.9.6-2.1 1-3.4 1a6 6 0 0 1-5.6-4.2H3v2.6A10 10 0 0 0 12 22Z"/><path fill="#fbbc05" d="M6.4 13.9A6 6 0 0 1 6.1 12c0-.7.1-1.3.3-1.9V7.5H3A10 10 0 0 0 2 12c0 1.6.4 3.1 1 4.5l3.4-2.6Z"/><path fill="#ea4335" d="M12 6c1.5 0 2.8.5 3.8 1.5l2.9-2.9A9.7 9.7 0 0 0 12 2a10 10 0 0 0-9 5.5l3.4 2.6A6 6 0 0 1 12 6Z"/></svg>
                Continue with Google
              </Button>

              <div className="my-6 flex items-center gap-3"><Separator /><span className="shrink-0 text-xs text-muted-foreground">or continue with email</span><Separator /></div>

              <form className="grid gap-5" onSubmit={handleSubmit} onChange={authStore.clearError}>
                <div className="grid gap-2"><Label htmlFor="username">Username</Label><Input id="username" name="username" autoComplete="username" placeholder="Enter your username" className="h-10" maxLength={128} required /></div>
                <div className="grid gap-2">
                  <div className="flex items-center justify-between"><Label htmlFor="password">Password</Label><a href="/forgot-password" className="text-xs font-medium underline-offset-4 hover:underline">Forgot password?</a></div>
                  <div className="relative"><Input id="password" name="password" type={showPassword ? "text" : "password"} autoComplete="current-password" placeholder="Enter your password" className="h-10 pr-10" maxLength={1024} required /><Button type="button" variant="ghost" size="icon" className="absolute right-0.5 top-0.5 size-9 text-muted-foreground" onClick={() => setShowPassword((value) => !value)} aria-label={showPassword ? "Hide password" : "Show password"}>{showPassword ? <EyeOff /> : <Eye />}</Button></div>
                </div>
                {authStore.error && <p role="alert" className="text-sm text-destructive">{authStore.error}</p>}
                <Label className="w-fit cursor-pointer text-xs font-normal text-muted-foreground"><Checkbox name="remember" />Remember me for 30 days</Label>
                <Button type="submit" className="h-10 w-full" disabled={authStore.isLoading}>{authStore.isLoading ? "Signing in…" : "Sign in"}</Button>
              </form>
              <p className="mt-7 text-center text-sm text-muted-foreground">Don&apos;t have an account? <a href="/sign-up" className="font-medium text-foreground underline-offset-4 hover:underline">Create an account</a></p>
            </CardContent>
          </Card>
        </div>
        <p className="text-center text-xs text-muted-foreground">By continuing, you agree to our <a className="underline underline-offset-4" href="/terms">Terms</a> and <a className="underline underline-offset-4" href="/privacy">Privacy Policy</a>.</p>
      </section>

      <aside className="relative m-3 hidden overflow-hidden rounded-2xl bg-neutral-950 p-12 text-white lg:flex lg:flex-col lg:justify-center xl:p-20">
        <div className="absolute inset-0 opacity-30 [background-image:linear-gradient(rgba(255,255,255,.08)_1px,transparent_1px),linear-gradient(90deg,rgba(255,255,255,.08)_1px,transparent_1px)] [background-size:44px_44px]" />
        <div className="absolute -right-32 -top-32 size-[420px] rounded-full bg-violet-600/30 blur-3xl" />
        <div className="relative max-w-xl">
          <span className="inline-flex items-center gap-2 rounded-full border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-neutral-300"><i className="size-1.5 rounded-full bg-emerald-400 ring-4 ring-emerald-400/10" />Live transcription</span>
          <h2 className="mt-6 text-5xl font-semibold leading-[1.02] tracking-[-0.055em] xl:text-6xl">Every word, right when you need it.</h2>
          <p className="mt-5 max-w-lg leading-7 text-neutral-400">Real-time subtitles and translations that make every conversation clear, accessible, and easy to follow.</p>
          <Card className="mt-12 border-white/10 bg-white/[.06] text-white shadow-2xl backdrop-blur-xl">
            <CardHeader className="flex flex-row items-center justify-between border-b border-white/10 py-4"><span className="text-xs tracking-[.3em] text-neutral-600">● ● ●</span><span className="flex items-center gap-2 text-xs text-neutral-400"><i className="size-1.5 rounded-full bg-emerald-400" />Listening</span></CardHeader>
            <CardContent className="pt-6">
              <div className="flex items-center gap-3"><span className="grid size-9 place-items-center rounded-lg bg-violet-600 text-xs font-semibold">AM</span><span className="grid"><b className="text-xs font-medium">Alex Morgan</b><small className="text-neutral-500">Just now</small></span></div>
              <p className="my-5 text-lg leading-7">Let&apos;s make every conversation feel effortless, no matter the language.</p>
              <div className="rounded-r-lg border-l-2 border-violet-500 bg-violet-500/10 p-4 text-xs leading-5 text-neutral-300"><span className="mb-1 flex items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wide text-violet-400"><Languages className="size-3" />Translated to Spanish</span>Hagamos que cada conversación sea sencilla, sin importar el idioma.</div>
            </CardContent>
          </Card>
          <p className="mt-8 text-sm text-neutral-300">“Suntitle helps our global team stay perfectly in sync.”<span className="block text-neutral-500">— Maya Chen, Product Lead</span></p>
        </div>
      </aside>
    </main>
  )
})

export default App
