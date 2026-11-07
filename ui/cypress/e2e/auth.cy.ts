const admin = { id: "11111111-1111-4111-8111-111111111111", username: "admin" }

function submitCredentials(password: string) {
  cy.get("#username").type(admin.username)
  cy.get("#password").type(password)
  cy.contains("button", "Sign in").click()
}

describe("Administrator login", () => {
  beforeEach(() => {
    cy.visit("/", { onBeforeLoad(window) { window.localStorage.clear() } })
  })

  it("shows invalid credentials without storing a session", () => {
    cy.intercept("POST", "**/api/v1/auth/login", {
      statusCode: 401,
      body: { type: "about:blank", title: "Unauthorized", status: 401, detail: "Invalid username or password." },
    }).as("login")
    submitCredentials("wrong-password")
    cy.wait("@login").its("request.body").should("deep.equal", { username: admin.username, password: "wrong-password" })
    cy.contains("Invalid username or password.").should("be.visible")
    cy.contains("button", "Sign in").should("be.enabled")
    cy.window().then((window) => {
      expect(window.localStorage.getItem("suntitle.accessToken")).to.equal(null)
    })
  })

  it("stores the administrator session and restores it after reload", () => {
    cy.intercept("POST", "**/api/v1/auth/login", {
      body: { accessToken: "admin-token", tokenType: "Bearer", expiresIn: 86400, user: admin },
    }).as("login")
    cy.intercept("GET", "**/api/v1/auth/me", { body: admin }).as("me")
    cy.intercept("GET", "**/api/v1/hosts", { body: { items: [] } })
    cy.intercept("GET", "**/api/v1/extensions", { body: { items: [] } })
    submitCredentials("admin-password")
    cy.wait("@login").its("request.body").should("deep.equal", { username: admin.username, password: "admin-password" })
    cy.location("pathname").should("equal", "/hosts")
    cy.window().then((window) => {
      expect(window.localStorage.getItem("suntitle.accessToken")).to.equal("admin-token")
    })
    cy.reload()
    cy.wait("@me").its("request.headers.authorization").should("equal", "Bearer admin-token")
    cy.get("h1").should("have.text", "Hosts")
  })
})
