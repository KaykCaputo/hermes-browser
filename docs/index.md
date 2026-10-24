---
title: Hermes Browser Documentation
description: Learn how to use, build, test, and contribute to Hermes Browser.
hide:
  - navigation
  - toc
---

<div class="hermes-home">

<section class="hermes-hero">
  <div class="hermes-hero__copy">
    <div class="hermes-brand">
      <img src="assets/hermes-browser.svg" alt="" aria-hidden="true">
      <span>Hermes Browser</span>
    </div>
    <h1>A focused browser, built on a different engine.</h1>
    <p class="hermes-hero__lead">Hermes pairs Servo's independent web engine with a lightweight native Rust interface. Explore the browser today, or help shape the path toward a faster and more open web.</p>
    <div class="hermes-actions">
      <a href="user-guide/installation/" class="md-button md-button--primary">Get started</a>
      <a href="developer-guide/" class="md-button">Developer guide</a>
    </div>
  </div>
  <div class="hermes-hero__visual">
    <img src="screenshots/hermes-new-tab.png" alt="Hermes Browser new tab page" loading="eager">
  </div>
</section>

<div class="hermes-proof">
  <div class="hermes-proof__item">
    <strong>Rust from interface to engine</strong>
    <span>Memory-safe foundations across the browser stack</span>
  </div>
  <div class="hermes-proof__item">
    <strong>Independent rendering</strong>
    <span>Powered by Servo, WebRender, and Vello</span>
  </div>
  <div class="hermes-proof__item">
    <strong>Native Linux media</strong>
    <span>Audio and video through GStreamer</span>
  </div>
</div>

<section class="hermes-section">
  <div class="hermes-section__intro">
    <p class="hermes-eyebrow">One project, two paths</p>
    <h2>Find the documentation written for you.</h2>
    <p>You should not need to understand Servo internals to run Hermes, and you should not need to search user tutorials for architectural decisions. Both guides live in one searchable site while staying focused on their audience.</p>
  </div>

  <div class="hermes-audience-grid">
    <article class="hermes-card">
      <span class="hermes-card__number">USER GUIDE</span>
      <h3>Browse with clear expectations</h3>
      <p>Install Hermes, learn its tabs and shortcuts, set up Linux media, and understand today's privacy and compatibility boundaries.</p>
      <a href="user-guide/">Explore the User Guide →</a>
    </article>
    <article class="hermes-card">
      <span class="hermes-card__number">DEVELOPER GUIDE</span>
      <h3>Understand the browser as a system</h3>
      <p>Follow commands and frames through the codebase, work safely around Servo, test changes, and maintain the project's focused patches.</p>
      <a href="developer-guide/">Explore the Developer Guide →</a>
    </article>
  </div>
</section>

<section class="hermes-section">
  <div class="hermes-section__intro">
    <p class="hermes-eyebrow">Built with purpose</p>
    <h2>A small browser shell with serious foundations.</h2>
  </div>

  <div class="hermes-feature-grid">
    <article class="hermes-card">
      <span class="hermes-card__number">01</span>
      <h3>Servo-powered browsing</h3>
      <p>Modern web content runs through an embeddable engine written for parallelism and memory safety.</p>
    </article>
    <article class="hermes-card">
      <span class="hermes-card__number">02</span>
      <h3>Native desktop experience</h3>
      <p>Slint, winit, and WGPU provide responsive tabs, navigation, keyboard input, and IME composition.</p>
    </article>
    <article class="hermes-card">
      <span class="hermes-card__number">03</span>
      <h3>Blocking before load</h3>
      <p>Brave's default filter snapshot stops known advertising and tracking subresources at the network boundary.</p>
    </article>
    <article class="hermes-card">
      <span class="hermes-card__number">04</span>
      <h3>Linux multimedia</h3>
      <p>Servo's GStreamer backend handles supported HTML audio, video, Web Audio, and WebRTC pipelines.</p>
    </article>
    <article class="hermes-card">
      <span class="hermes-card__number">05</span>
      <h3>Engine-neutral core</h3>
      <p>Talaria keeps tabs, state, commands, and navigation policy testable without starting a browser engine.</p>
    </article>
    <article class="hermes-card">
      <span class="hermes-card__number">06</span>
      <h3>Honest engineering</h3>
      <p>Known limits are documented directly, including site compatibility and the current lack of OS sandboxing.</p>
    </article>
  </div>
</section>

<section class="hermes-showcase">
  <img src="screenshots/hermes-servo.png" alt="The Servo website rendered in a multi-tab Hermes window" loading="lazy">
  <div class="hermes-showcase__copy">
    <p class="hermes-eyebrow">A real browser, in progress</p>
    <h2>Native chrome. Independent web engine.</h2>
    <p>Every tab owns a Servo WebView. Hermes routes native input into the engine and brings page titles, favicons, history, loading state, and rendered frames back into a focused desktop experience.</p>
    <a href="developer-guide/architecture/">See how the layers fit together →</a>
  </div>
</section>

<aside class="hermes-preview">
  <img class="hermes-preview__mark" src="assets/hermes-browser.svg" alt="Hermes Browser logo">
  <div>
    <h2>Hermes is an engineering preview.</h2>
    <p>Web content runs in isolated content processes without operating-system sandbox restrictions. Use known sites and non-sensitive data while that security boundary is hardened.</p>
  </div>
  <a href="user-guide/privacy-and-security/">Read the security model →</a>
</aside>

</div>
