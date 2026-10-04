const accessToken = "mock-token"

function visitAuthenticatedExtensions() {
  cy.intercept("GET", "**/api/v1/auth/me", { fixture: "api/v1/auth/me.json" }).as("getCurrentUser")
  cy.intercept("GET", "**/api/v1/extensions", { fixture: "api/v1/extensions.json" }).as("getExtensions")

  cy.visit("/extensions", {
    onBeforeLoad(window) {
      window.localStorage.setItem("suntitle.accessToken", accessToken)
    },
  })
}

describe("Extensions page", () => {
  it("lists extensions and refreshes one from its actions dropdown", () => {
    cy.intercept("POST", "**/api/v1/extensions/obs-captions/refresh", {
      statusCode: 202,
      body: {
        name: "obs-captions",
        version: "1.5.0",
        description: "Publishes live subtitles to OBS Studio.",
        source: "https://github.com/suntitle/extensions/obs-captions.git",
        sourceType: "git",
      },
    }).as("refreshExtension")

    visitAuthenticatedExtensions()
    cy.wait(["@getCurrentUser", "@getExtensions"])

    cy.get("h1").should("have.text", "Extensions")
    cy.contains("2 extensions").should("be.visible")
    cy.contains("tr", "obs-captions").within(() => {
      cy.contains("1.4.2").should("be.visible")
      cy.contains("Git").should("be.visible")
      cy.get('button[aria-label="Actions for obs-captions"]').click()
    })
    cy.contains('[data-slot="dropdown-menu-item"]', "Refresh extension").click()

    cy.wait("@refreshExtension").its("request.headers.authorization").should("equal", `Bearer ${accessToken}`)
    cy.contains("tr", "obs-captions").should("contain.text", "1.5.0")
  })

  it("installs an extension from a local file", () => {
    cy.intercept("POST", "**/api/v1/extensions", {
      statusCode: 201,
      body: {
        name: "custom-captioner",
        version: "1.0.0",
        description: "Custom caption processing.",
        source: "custom-captioner.zip",
        sourceType: "local",
      },
    }).as("installExtension")

    visitAuthenticatedExtensions()
    cy.wait("@getExtensions")
    cy.contains("button", "Install extension").click()

    cy.get('[data-slot="dialog-title"]').should("have.text", "Install extension")
    cy.get('button[aria-label="Source type"]').should("contain.text", "Local")
    cy.get('input[type="file"]').selectFile({
      contents: Cypress.Buffer.from("extension"),
      fileName: "custom-captioner.zip",
      mimeType: "application/zip",
    })
    cy.contains('[data-slot="dialog-content"] button', "Install extension").click()

    cy.wait("@installExtension").then(({ request }) => {
      expect(request.headers.authorization).to.equal(`Bearer ${accessToken}`)
      expect(request.body).to.deep.equal({ sourceType: "local", source: "custom-captioner.zip" })
    })
    cy.get('[data-slot="dialog-content"]').should("not.exist")
    cy.contains("tr", "custom-captioner").should("be.visible")
    cy.contains("3 extensions").should("be.visible")
  })

  it("installs an extension from a Git URL", () => {
    cy.intercept("POST", "**/api/v1/extensions", {
      statusCode: 201,
      body: {
        name: "community-translator",
        version: "0.3.0",
        description: "Community translation provider.",
        source: "https://github.com/example/community-translator.git",
        sourceType: "git",
      },
    }).as("installExtension")

    visitAuthenticatedExtensions()
    cy.wait("@getExtensions")
    cy.contains("button", "Install extension").click()
    cy.get('button[aria-label="Source type"]').click()
    cy.contains('[data-slot="select-item"]', "Git").click()
    cy.get("#extension-source").type("https://github.com/example/community-translator.git")
    cy.contains('[data-slot="dialog-content"] button', "Install extension").click()

    cy.wait("@installExtension").its("request.body").should("deep.equal", {
      sourceType: "git",
      source: "https://github.com/example/community-translator.git",
    })
  })
})
