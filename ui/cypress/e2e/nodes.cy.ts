const accessToken = "mock-token"

function visitAuthenticatedWorkspace() {
  cy.intercept("GET", "**/api/v1/auth/me", {
    fixture: "api/v1/auth/me.json",
  }).as("getCurrentUser")
  cy.intercept("GET", "**/api/v1/hosts", {
    fixture: "api/v1/hosts.json",
  }).as("getHosts")
  cy.intercept("GET", "**/api/v1/nodes", {
    fixture: "api/v1/nodes.json",
  }).as("getNodes")

  cy.visit("/", {
    onBeforeLoad(window) {
      window.localStorage.setItem("suntitle.accessToken", accessToken)
    },
  })
}

describe("Nodes page", () => {
  it("uses the Nodes route and renders unrelated pipelines separately", () => {
    visitAuthenticatedWorkspace()
    cy.wait(["@getCurrentUser", "@getHosts"])

    cy.contains("a", "Nodes").click()
    cy.location("pathname").should("equal", "/nodes")
    cy.wait("@getNodes")
      .its("request.headers.authorization")
      .should("equal", `Bearer ${accessToken}`)

    cy.get("h1").should("have.text", "Nodes").and("be.visible")
    cy.contains("7 nodes").should("be.visible")
    cy.get('[data-testid="topology-pipeline"]').should("have.length", 2)

    cy.get('[data-testid="topology-pipeline"]').eq(0).within(() => {
      cy.contains("Pipeline A").should("be.visible")
      cy.contains("Studio microphone").should("be.visible")
      cy.contains("Live transcriber").should("be.visible")
      cy.contains("English → Spanish").should("exist")
      cy.contains("Stage display").scrollIntoView().should("be.visible")
      cy.get('[data-testid="topology-connection"]').should("have.length", 3)
    })

    cy.get('[data-testid="topology-pipeline"]').eq(1).within(() => {
      cy.contains("Pipeline B").should("be.visible")
      cy.contains("Lobby microphone").should("be.visible")
      cy.contains("Lobby transcriber").should("be.visible")
      cy.contains("Lobby display").scrollIntoView().should("be.visible")
      cy.get('[data-testid="topology-connection"]').should("have.length", 2)
    })

    cy.reload()
    cy.wait(["@getCurrentUser", "@getNodes"])
    cy.location("pathname").should("equal", "/nodes")
    cy.get("h1").should("have.text", "Nodes")
  })
})
