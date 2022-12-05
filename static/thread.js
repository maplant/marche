$(document).ready(function() {
    $(".edit-post-button").click(function() {
        const id = $(this).attr("postid");
        $(`.unparsed-${id}`).slideToggle();
        $(`.parsed-${id}`).slideToggle();
    });

    // editing replies
    $(".edit-post-form").each(function() {
        const id = $(this).attr("postid");
        const thread_id = $(this).attr("threadid");
        $(this).ajaxForm({
            url: `/reply/${id}`,
            type: 'post',
            success: function(response, _, _, _) {
                if (response.error !== undefined) {
                    $(`.error-${id}`).show();
                    $(`.error-${id}`).html(`${response.error}`)
                } else {
                    // TODO: Do this properly
                    location.href = `/thread/${thread_id}?jump_to=${id}`;
                }
            },
            error: function(_, status, error) {
                  $(`.error-${id}`).html("Error attempting to update post")
            }
        });
    });

    // Replace @ with responses
    const has_reply_preview = new Map();
    $(".post-text").each(function() {        
        const REPLY_RE = /@(\d+)/g;
        let html = $(this).html();
        let curr_id = $(this).parents(".reply").attr("id");
        $(this).html(html.replaceAll(REPLY_RE, function(match, id, _, _, _) {
            let post = $(`#${id}`);
            let author = post.attr("author");
            if (id < curr_id && post.length) {
                return `<span class="respond-to-preview" reply_id=${id}><b>@${author}</b></span><div class="overlay-on-hover reply-overlay"></div>`;
            } else {
                return match;
            }
        }));
    });

    $("span.respond-to-preview").each(function() {
        var response_div = $(this).parents(".reply");
        var response_div_clone = response_div.clone();
        var responder_name = response_div.attr("author");
        var response_preview_div = $(
            $.parseHTML(
                `<div class="response-from-preview action-box" reply_id="{reply_id}"><b>🗣️ ${responder_name}</b></div>`,
            ),
        );
        var response_overlay_div = $(
            $.parseHTML(
                `<div class="response-from-preview response-overlay overlay-on-hover" style="display: inline-block;"></div>`,
            ),
        );
        var response_container_div = getResponseContainerDiv($(this));

        cleanCloneDiv(response_div_clone);

        response_preview_div.hover(function() {
            response_overlay_div[0].replaceChildren(response_div_clone[0]);
            response_overlay_div.css("visibility", "visible").css("opacity", "1.0");
        }, function() {
            response_overlay_div.css("visibility", "hidden").css("opacity", "0.0");
        });
        response_preview_div.click(function() {
            response_div[0].scrollIntoView({ behavior: "smooth", block: "center" });
        });
        response_overlay_div[0].appendChild(response_div_clone[0]);
        response_container_div.parent().append(response_overlay_div);
        response_container_div.append(response_preview_div);
    });

    $(".reply-to-button").click(function() {
        $("#reply")[0].scrollIntoView({ behavior: "smooth" });
        $("#reply-textarea")[0].value += `@${$(this).attr("replyid")} `
    });

    $(".respond-to-preview").click(function() {
        getReplyDiv($(this))[0].scrollIntoView({ behavior: "smooth", block: "center" });
    });

    $(".respond-to-preview").hover(function() {
        var overlay_div = getOverlayDiv($(this));
        var reply_div_clone = getReplyDiv($(this)).clone();
        cleanCloneDiv(reply_div_clone);
        overlay_div[0].replaceChildren(reply_div_clone[0]);
        overlay_div.css("visibility", "visible").css("opacity", "1.0");
    }, function() {
        getOverlayDiv($(this)).css("visibility", "hidden").css("opacity", "0.0");
    });

    // Embed media elements
    $("a").each(function() {
        const YOUTUBE_RE =
            /(?:https?:\/\/)?(?:www\.|m\.)?youtu(?:\.be\/|be.com\/\S*(?:watch|embed)(?:(?:(?=\/[^&\s\?]+(?!\S))\/)|(?:\S*v=|v\/)))([^&\s\?]+)/;

        const link = $(this).attr("href");
        const capture = link.match(YOUTUBE_RE);
        if (capture != null) {
            const id = capture[1];
            $(this).replaceWith(
                $(`<p><iframe width="560" height="315" src="https://www.youtube.com/embed/${id}" title="YouTube video player" frameborder="0" allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture" allowfullscreen></iframe></p>`),
            );
            return;
        }
    });

    // Check if jump_to exists, and scroll to it if it does
    const urlParams = new URLSearchParams(window.location.search);
    if (urlParams.has('jump_to')) {
        const jump_to = urlParams.get('jump_to');
        $(`#${jump_to}`)[0].scrollIntoView({ block: "center" });
    }

    // Custom file input button
    $("#attach-file-to-reply-input").change(function(event) {
        var file = event.target.files[0];
        var button = $(this).parents("#attach-file-to-reply-button");
        var buttonTextHolder = $("#attach-file-to-reply-text-container");
        var filenameTextHolder = $("#attached-filename-text-container");
        if (file) {
            button.attr("title", file.name);
            button.css("background-color", "lightgreen");
            buttonTextHolder[0].textContent="✔️ File!";
            filenameTextHolder[0].textContent=`└ ${file.name}`;
        } else {
            button.attr("title", "");
            button.css("background-color", "");
            buttonTextHolder[0].textContent="+ File!";
            filenameTextHolder[0].textContent="";
        }
      });
});

function cleanCloneDiv(div) {
    // Differences between response to preview and actual reply element
    div.removeAttr("id");
    div.find(".edit-post-button").remove();
    div.find(".hide-post").remove();
    div.find(".delete-reply").remove();
    div.find(".reply-to-button").remove();
    div.find(".react-button").remove();
    div.find(".edit-post-form").remove();
}

function getOverlayDiv(origin) {
    var overlay_div = origin.next("div.reply-overlay");

    // Workaround because markdown parser doesn't close its own <p> tags.  garbage.
    if (overlay_div.length == 0) {
        overlay_div = origin.parent().next("div.reply-overlay");
    }
    return overlay_div;
}

function getReplyDiv(origin) {
    // If we ever add paginated threads, this logic needs to be extended in order to retrieve/render replys which are not in the DOM
    return $(`#${origin.attr("reply_id")}`);
}

function getResponseContainerDiv(origin) {
    // If we ever add paginated threads, this logic needs to be extended in order to retrieve/render replys which are not in the DOM
    return $(`#response-container-${origin.attr("reply_id")}`);
}
