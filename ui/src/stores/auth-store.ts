import { makeAutoObservable, runInAction } from "mobx"
import type { ApiClient } from "./api-client"
import { ApiError, getErrorMessage } from "./api-client"
import type { LoginRequest, LoginResponse, User } from "./models"

const TOKEN_STORAGE_KEY = "suntitle.accessToken"

export class AuthStore {
  accessToken: string | null = localStorage.getItem(TOKEN_STORAGE_KEY)
  user: User | null = null
  expiresAt: number | null = null
  isInitialized = false
  isCheckingSession = false
  isLoading = false
  error: string | null = null

  constructor(private readonly api: ApiClient) {
    makeAutoObservable<this, "api">(this, { api: false }, { autoBind: true })
  }

  get isAuthenticated(): boolean {
    return this.accessToken !== null && this.user !== null && (this.expiresAt === null || this.expiresAt > Date.now())
  }

  async initialize(): Promise<void> {
    if (this.isInitialized || this.isCheckingSession) return

    this.isCheckingSession = true
    this.error = null

    if (this.accessToken === null) {
      this.isCheckingSession = false
      this.isInitialized = true
      return
    }

    try {
      const user = await this.api.get<User>("/auth/me")
      runInAction(() => { this.user = user })
    } catch (error) {
      runInAction(() => {
        this.clearSession()
        if (!(error instanceof ApiError && error.status === 401)) {
          this.error = getErrorMessage(error)
        }
      })
    } finally {
      runInAction(() => {
        this.isCheckingSession = false
        this.isInitialized = true
      })
    }
  }

  async login(credentials: LoginRequest): Promise<void> {
    this.isLoading = true
    this.error = null
    try {
      const response = await this.api.post<LoginResponse>("/auth/login", credentials)
      runInAction(() => {
        this.accessToken = response.accessToken
        this.user = response.user
        this.expiresAt = Date.now() + response.expiresIn * 1000
        this.isInitialized = true
        localStorage.setItem(TOKEN_STORAGE_KEY, response.accessToken)
      })
    } catch (error) {
      runInAction(() => { this.error = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => { this.isLoading = false })
    }
  }

  async logout(): Promise<void> {
    this.isLoading = true
    this.error = null
    try {
      await this.api.post<void>("/auth/logout")
    } catch (error) {
      runInAction(() => { this.error = getErrorMessage(error) })
      throw error
    } finally {
      runInAction(() => {
        this.clearSession()
        this.isLoading = false
      })
    }
  }

  clearError(): void { this.error = null }

  clearSession(): void {
    this.accessToken = null
    this.user = null
    this.expiresAt = null
    localStorage.removeItem(TOKEN_STORAGE_KEY)
  }
}
