function rustup_text_unix(host, platform) {
    return `wget <a href="${host}/rustup/dist/${platform}/rustup-init">${host}/rustup/dist/${platform}/rustup-init</a>
chmod +x rustup-init
./rustup-init`
}

function rustup_text_win(host, platform) {
    return `Download rustup-init.exe here:
<a href="${host}/rustup/dist/${platform}/rustup-init.exe">${host}/rustup/dist/${platform}/rustup-init.exe</a>`
}

function platform_change() {
    let rustup_text = document.getElementById("rustup-text");
    let rustup_platform = document.getElementById("rustup-selected-platform");
    let host = document.getElementById("panamax-host").textContent;
    let platform = rustup_platform.options[rustup_platform.selectedIndex].text;
    let is_exe = rustup_platform.options[rustup_platform.selectedIndex].value;
    if (is_exe === "true") {
        rustup_text.innerHTML = rustup_text_win(host, platform);
    } else {
        rustup_text.innerHTML = rustup_text_unix(host, platform);
    }
}