const accessToken = "mock-token"

function visitAuthenticatedHosts() {
  cy.intercept("GET", "**/api/v1/auth/me", {
    fixture: "api/v1/auth/me.json",
  }).as("getCurrentUser")

  cy.visit("/", {
    onBeforeLoad(window) {
      window.localStorage.setItem("suntitle.accessToken", accessToken)
    },
  })
}

describe("Hosts page", () => {
  it("renders hosts returned by the API", () => {
    cy.intercept("GET", "**/api/v1/hosts", {
      fixture: "api/v1/hosts.json",
    }).as("getHosts")

    visitAuthenticatedHosts()

    cy.wait("@getCurrentUser")
      .its("request.headers.authorization")
      .should("equal", `Bearer ${accessToken}`)
    cy.wait("@getHosts")
      .its("request.headers.authorization")
      .should("equal", `Bearer ${accessToken}`)

    cy.get("h1").should("have.text", "Hosts").and("be.visible")
    cy.contains("1 host").should("be.visible")
    cy.contains("tr", "studio-host").within(() => {
      cy.contains("192.0.2.10").should("be.visible")
      cy.contains("0.1.0").should("be.visible")
      cy.contains("online").should("be.visible")
    })
  })

  it("shows the empty state when no hosts are registered", () => {
    cy.intercept("GET", "**/api/v1/hosts", {
      body: { items: [], total: 0 },
    }).as("getHosts")

    visitAuthenticatedHosts()
    cy.wait("@getHosts")

    cy.contains("0 hosts").should("be.visible")
    cy.contains("No hosts are available yet.").should("be.visible")
  })

  it("shows the API problem detail when loading hosts fails", () => {
    cy.intercept("GET", "**/api/v1/hosts", {
      statusCode: 503,
      headers: { "content-type": "application/problem+json" },
      body: {
        type: "about:blank",
        title: "Service unavailable",
        status: 503,
        detail: "Hosts are temporarily unavailable.",
      },
    }).as("getHosts")

    visitAuthenticatedHosts()
    cy.wait("@getHosts")

    cy.contains("td", "Hosts are temporarily unavailable.").should("be.visible")
  })
})
