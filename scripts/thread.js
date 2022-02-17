$(document).ready(function() {
    var easymde = new EasyMDE({ status: false, lineWrapping: true });
    $(".editor-toolbar").css("border", "none");

    $(".reply-to-button").click(function() {
        $("#reply")[0].scrollIntoView({ behavior: "smooth" });
        insertAtCurrentLine(easymde, ` @${$(this).attr("replyid")}`);
    });

    $(".respond-to-preview").click(function() {
        getReplyDiv($(this))[0].scrollIntoView({ behavior: "smooth", block: "center" });
    });

    $(".respond-to-preview").hover(function() {
        var overlay_div = getOverlayDiv($(this));

        var reply_div_clone = getReplyDiv($(this)).clone();

        // Differences between response to preview and actual reply element
        reply_div_clone.removeAttr("id");
        reply_div_clone.find(".reply-to-button").remove();
        reply_div_clone.find(".react-button").remove();

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
                $(`<iframe width="560" height="315" src="https://www.youtube.com/embed/${id}" title="YouTube video player" frameborder="0" allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture" allowfullscreen></iframe>`),
            );
            return;
        }
    });

    // Populate response elements
    $("div.respond-to-preview").each(function() {
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

        // Differences between response from preview and actual reply element
        response_div_clone.find(".response-container").removeAttr("id");
        response_div_clone.find(".reply-to-button").remove();
        response_div_clone.find(".react-button").remove();
        response_div_clone.removeAttr("id");

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
});

function insertAtCurrentLine(mde, text) {
    cm = mde.codemirror;
    var doc = cm.getDoc();
    var cursor = doc.getCursor();
    var line = doc.getLine(cursor.line);
    var pos = {
        line: cursor.line,
        ch: line.length,
    };
    doc.replaceRange(text, pos);
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
