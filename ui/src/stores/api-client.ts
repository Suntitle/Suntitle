import type { Problem } from "./models"

const DEFAULT_BASE_URL = "/api/v1"

export class ApiError extends Error {
  readonly status: number
  readonly problem?: Problem

  constructor(status: number, message: string, problem?: Problem) {
    super(message)
    this.name = "ApiError"
    this.status = status
    this.problem = problem
  }
}

export class ApiClient {
  private readonly baseUrl: string
  private readonly getAccessToken: () => string | null

  constructor(getAccessToken: () => string | null, baseUrl = DEFAULT_BASE_URL) {
    this.getAccessToken = getAccessToken
    this.baseUrl = baseUrl.replace(/\/$/, "")
  }

  get<T>(path: string): Promise<T> {
    return this.request<T>(path, { method: "GET" })
  }

  post<T>(path: string, body?: unknown): Promise<T> {
    return this.request<T>(path, { method: "POST", body: body === undefined ? undefined : JSON.stringify(body) })
  }

  put<T>(path: string, body: unknown): Promise<T> {
    return this.request<T>(path, { method: "PUT", body: JSON.stringify(body) })
  }

  private async request<T>(path: string, init: RequestInit): Promise<T> {
    const headers = new Headers(init.headers)
    const accessToken = this.getAccessToken()

    headers.set("Accept", "application/json, application/problem+json")
    if (init.body !== undefined) headers.set("Content-Type", "application/json")
    if (accessToken) headers.set("Authorization", `Bearer ${accessToken}`)

    const response = await fetch(`${this.baseUrl}${path}`, { ...init, headers })
    if (!response.ok) throw await this.createError(response)
    if (response.status === 204) return undefined as T

    return response.json() as Promise<T>
  }

  private async createError(response: Response): Promise<ApiError> {
    const contentType = response.headers.get("content-type") ?? ""
    let problem: (Problem & { error?: { message?: string } }) | undefined

    if (contentType.includes("json")) {
      try {
        problem = await response.json() as Problem
      } catch {
        problem = undefined
      }
    }

    const message = problem?.error?.message ?? problem?.detail ?? problem?.title ?? `Request failed with status ${response.status}`
    return new ApiError(response.status, message, problem)
  }
}

export function getErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "An unexpected error occurred"
}
